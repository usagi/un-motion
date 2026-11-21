//! Deleted profile snapshots include their owned inputs; existing launch defaults are preserved.
use serde::{Deserialize, Serialize};
use std::{
	collections::HashSet,
	fs,
	io::Write,
	path::Path,
	sync::atomic::{AtomicU64, Ordering},
	time::{SystemTime, UNIX_EPOCH},
};
use un_motion_profile_schema::{CoreProfileDocument, CoreProfileDocumentProfile, CoreProfileDocumentSource};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArchivedProfile {
	version: u32,
	pub profile: CoreProfileDocumentProfile,
	pub sources: Vec<CoreProfileDocumentSource>,
}

impl ArchivedProfile {
	pub fn revision(&self) -> String {
		use sha2::{Digest, Sha256};
		let bytes = serde_json::to_vec(self).expect("serializable profile archive");
		format!("delete:{:x}", Sha256::digest(bytes))
	}
	pub fn capture(doc: &CoreProfileDocument, id: &str) -> Result<Self, String> {
		let profile = doc
			.profiles
			.iter()
			.find(|p| p.id == id)
			.cloned()
			.ok_or_else(|| format!("profile not found: {id}"))?;
		let snapshot = Self {
			version: 1,
			profile,
			sources: doc.profile_sources.iter().filter(|s| s.profile_id == id).cloned().collect(),
		};
		snapshot.validate()?;
		Ok(snapshot)
	}

	fn validate(&self) -> Result<(), String> {
		if self.version != 1 || self.profile.id.is_empty() {
			return Err("invalid or unsupported profile archive".into());
		}
		let mut ids = HashSet::new();
		for source in &self.sources {
			if source.profile_id != self.profile.id || source.id.is_empty() || !ids.insert(&source.id) {
				return Err("invalid profile archive input ownership or duplicate input ID".into());
			}
		}
		Ok(())
	}

	pub fn restore_into(&self, doc: &mut CoreProfileDocument) -> Result<(), String> {
		self.validate()?;
		if doc.profiles.iter().any(|p| p.id == self.profile.id) {
			return Err(format!("profile already exists: {}", self.profile.id));
		}
		if doc
			.profile_sources
			.iter()
			.any(|s| s.profile_id == self.profile.id || self.sources.iter().any(|a| a.id == s.id))
		{
			return Err("profile archive conflicts with existing inputs".into());
		}
		doc.profiles.push(self.profile.clone());
		doc.profile_sources.extend(self.sources.clone());
		Ok(())
	}
}

static NEXT_ARCHIVE: AtomicU64 = AtomicU64::new(0);

pub fn store(root: &Path, snapshot: &ArchivedProfile) -> Result<String, String> {
	snapshot.validate()?;
	let bytes = serde_json::to_vec(snapshot).map_err(|e| e.to_string())?;
	fs::create_dir_all(root).map_err(|e| e.to_string())?;
	let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
	let id = format!("{}-{nanos}-{}", std::process::id(), NEXT_ARCHIVE.fetch_add(1, Ordering::Relaxed));
	let temporary = root.join(format!("{id}.tmp"));
	let destination = root.join(format!("{id}.json"));
	let result = (|| -> std::io::Result<()> {
		let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
		file.write_all(&bytes)?;
		file.sync_all()?;
		drop(file);
		fs::hard_link(&temporary, &destination)?;
		Ok(())
	})();
	let _ = fs::remove_file(&temporary);
	result.map_err(|e| format!("save profile archive: {e}"))?;
	Ok(id)
}

pub fn load(root: &Path, id: &str) -> Result<ArchivedProfile, String> {
	if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
		return Err("invalid archive ID".into());
	}
	let bytes = fs::read(root.join(format!("{id}.json"))).map_err(|e| e.to_string())?;
	let snapshot: ArchivedProfile = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
	snapshot.validate()?;
	Ok(snapshot)
}

#[derive(Debug, Serialize)]
pub struct ArchiveSummary {
	pub archive_id: String,
	pub profile_id: String,
	pub name: String,
}

pub fn list(root: &Path) -> Result<Vec<ArchiveSummary>, String> {
	let entries = match fs::read_dir(root) {
		Ok(entries) => entries,
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
		Err(error) => return Err(error.to_string()),
	};
	let mut result = Vec::new();
	for entry in entries {
		let path = entry.map_err(|e| e.to_string())?.path();
		if path.extension().and_then(|s| s.to_str()) != Some("json") {
			continue;
		}
		let id = path.file_stem().and_then(|s| s.to_str()).ok_or("invalid archive filename")?;
		let snapshot = load(root, id)?;
		result.push(ArchiveSummary {
			archive_id: id.into(),
			profile_id: snapshot.profile.id,
			name: snapshot.profile.name,
		});
	}
	result.sort_by(|a, b| b.archive_id.cmp(&a.archive_id));
	Ok(result)
}

pub fn restore_saved(store: &un_motion_profile_schema::CoreProfileDocumentStore, id: &str) -> Result<CoreProfileDocument, String> {
	let root = store
		.conf_path()
		.parent()
		.ok_or("profile store has no parent")?
		.join("deleted_profiles");
	let snapshot = load(&root, id)?;
	// The store selects the only profile when restoring into an empty library.
	store
		.update(|doc| snapshot.restore_into(doc).map_err(anyhow::Error::msg))
		.map_err(|e| format!("save restored profile: {e}"))
}

/// Caller checks runtime usage; snapshot and removal share the profile-store lock.
pub fn delete_saved(
	store: &un_motion_profile_schema::CoreProfileDocumentStore,
	id: &str,
	expected_revision: Option<&str>,
) -> Result<(), String> {
	let root = store
		.conf_path()
		.parent()
		.ok_or("profile store has no parent")?
		.join("deleted_profiles");
	store
		.update(|doc| {
			let snapshot = ArchivedProfile::capture(doc, id).map_err(anyhow::Error::msg)?;
			if expected_revision.is_some_and(|expected| {
				expected
					!= if expected.starts_with("delete:") {
						snapshot.revision()
					} else {
						crate::profile_revision(&snapshot.profile)
					}
			}) {
				anyhow::bail!("profile_conflict: profile changed since deletion was requested; reload and review before deleting");
			}
			self::store(&root, &snapshot).map_err(anyhow::Error::msg)?;
			doc.profiles.retain(|profile| profile.id != id);
			doc.profile_sources.retain(|source| source.profile_id != id);
			if doc.selected_profile_id == id {
				doc.selected_profile_id = doc.profiles.first().map(|p| p.id.clone()).unwrap_or_default();
			}
			Ok(())
		})
		.map_err(|e| format!("delete archived profile: {e}"))?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn deletion_revision_covers_owned_inputs_only() {
		let root = std::env::temp_dir().join(format!(
			"unm-delete-input-revision-{}-{}",
			std::process::id(),
			SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
		));
		let store = un_motion_profile_schema::CoreProfileDocumentStore::from_root(&root);
		store.save(fixture()).unwrap();
		let initial = store.load().unwrap();
		let revision = ArchivedProfile::capture(&initial, "a").unwrap().revision();
		let mut unrelated = initial.clone();
		let mut other = unrelated.profile_sources[0].clone();
		other.id = "other".into();
		other.profile_id = "b".into();
		unrelated.profile_sources.push(other);
		assert_eq!(ArchivedProfile::capture(&unrelated, "a").unwrap().revision(), revision);
		store
			.update(|doc| {
				doc.profile_sources[0].label = "Changed source".into();
				Ok(())
			})
			.unwrap();
		let changed = store.load().unwrap();
		assert_eq!(
			crate::profile_revision(&initial.profiles[0]),
			crate::profile_revision(&changed.profiles[0])
		);
		assert!(delete_saved(&store, "a", Some(&revision)).unwrap_err().contains("profile_conflict"));
		assert_eq!(store.load().unwrap(), changed);
		assert!(list(&root.join("deleted_profiles")).unwrap().is_empty());
		let reviewed = ArchivedProfile::capture(&changed, "a").unwrap().revision();
		delete_saved(&store, "a", Some(&reviewed)).unwrap();
		fs::remove_dir_all(root).unwrap();
	}
	#[test]
	fn stale_delete_preserves_changed_profile_and_creates_no_archive() {
		let root = std::env::temp_dir().join(format!(
			"unm-delete-revision-{}-{}",
			std::process::id(),
			SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
		));
		let store = un_motion_profile_schema::CoreProfileDocumentStore::from_root(&root);
		store.save(fixture()).unwrap();
		let before = store.load().unwrap();
		let revision = crate::profile_revision(&before.profiles[0]);
		store
			.update(|doc| {
				doc.profiles[0].name = "Changed elsewhere".into();
				Ok(())
			})
			.unwrap();
		let changed = store.load().unwrap();
		assert!(delete_saved(&store, "a", Some(&revision)).unwrap_err().contains("profile_conflict"));
		assert_eq!(store.load().unwrap(), changed);
		assert!(list(&root.join("deleted_profiles")).unwrap().is_empty());
		let reviewed = crate::profile_revision(&changed.profiles[0]);
		delete_saved(&store, "a", Some(&reviewed)).unwrap();
		let entries = list(&root.join("deleted_profiles")).unwrap();
		assert_eq!(entries.len(), 1);
		assert_eq!(
			load(&root.join("deleted_profiles"), &entries[0].archive_id).unwrap().profile,
			changed.profiles[0]
		);
		fs::remove_dir_all(root).unwrap();
	}
	#[test]
	fn transactional_delete_restore_and_archive_failure_preserve_data() {
		let root = std::env::temp_dir().join(format!(
			"unm-delete-transaction-{}-{}",
			std::process::id(),
			SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
		));
		let store = un_motion_profile_schema::CoreProfileDocumentStore::from_root(&root);
		store.save(fixture()).unwrap();
		let original = store.load().unwrap();
		let archives = root.join("deleted_profiles");
		fs::write(&archives, b"not a directory").unwrap();
		assert!(delete_saved(&store, "a", None).is_err());
		assert_eq!(store.load().unwrap(), original);
		fs::remove_file(&archives).unwrap();
		delete_saved(&store, "a", None).unwrap();
		assert!(store.load().unwrap().profiles.is_empty());
		let id = list(&archives).unwrap()[0].archive_id.clone();
		let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
		let workers: Vec<_> = (0..2)
			.map(|_| {
				let store = store.clone();
				let id = id.clone();
				let barrier = barrier.clone();
				std::thread::spawn(move || {
					barrier.wait();
					restore_saved(&store, &id)
				})
			})
			.collect();
		let outcomes: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
		assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
		let restored = store.load().unwrap();
		assert_eq!(restored.profiles, original.profiles);
		assert_eq!(restored.profile_sources, original.profile_sources);
		assert_eq!(list(&archives).unwrap().len(), 1);
		fs::remove_dir_all(root).unwrap();
	}
	fn fixture() -> CoreProfileDocument {
		let mut doc = CoreProfileDocument::default();
		doc.profiles.truncate(1);
		doc.profiles[0].id = "a".into();
		doc.profile_sources = vec![CoreProfileDocumentSource {
			id: "camera".into(),
			profile_id: "a".into(),
			kind: "webcam".into(),
			label: "Camera".into(),
			protocol: None,
			host: None,
			port: None,
			mirror_correction_enabled: true,
		}];
		doc
	}
	#[test]
	fn restores_owned_inputs_without_changing_default_or_other_profile() {
		let original = fixture();
		let snapshot = ArchivedProfile::capture(&original, "a").unwrap();
		let mut current = original.clone();
		current.profiles[0].id = "b".into();
		current.profile_sources.clear();
		current.selected_profile_id = "b".into();
		let before = current.clone();
		snapshot.restore_into(&mut current).unwrap();
		assert_eq!(current.profiles[0], before.profiles[0]);
		assert_eq!(current.profiles[1], original.profiles[0]);
		assert_eq!(current.profile_sources, original.profile_sources);
		assert_eq!(current.selected_profile_id, before.selected_profile_id);
		assert_eq!(current.next_profile_index, before.next_profile_index);
		assert_eq!(current.next_source_index, before.next_source_index);
	}
	#[test]
	fn conflicts_leave_document_unchanged() {
		let mut doc = fixture();
		let snapshot = ArchivedProfile::capture(&doc, "a").unwrap();
		let before = doc.clone();
		assert!(snapshot.restore_into(&mut doc).is_err());
		assert_eq!(doc, before);
		doc.profiles.clear();
		doc.profile_sources[0].profile_id = "b".into();
		let before = doc.clone();
		assert!(snapshot.restore_into(&mut doc).is_err());
		assert_eq!(doc, before);
	}
	#[test]
	fn real_store_restore_preserves_other_profile_and_rejects_repeat() {
		let root = std::env::temp_dir().join(format!(
			"unm-restore-store-{}-{}",
			std::process::id(),
			SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
		));
		let store = un_motion_profile_schema::CoreProfileDocumentStore::from_root(&root);
		let mut doc = fixture();
		let mut second = doc.profiles[0].clone();
		second.id = "b".into();
		second.name = "Second".into();
		doc.profiles.push(second);
		doc.selected_profile_id = "b".into();
		store.save(doc).unwrap();
		let original = store.load().unwrap();
		let snapshot = ArchivedProfile::capture(&original, "a").unwrap();
		let archives = root.join("deleted_profiles");
		let id = super::store(&archives, &snapshot).unwrap();
		let mut deleted = original.clone();
		deleted.profiles.retain(|p| p.id != "a");
		deleted.profile_sources.clear();
		store.save(deleted).unwrap();
		let restored = restore_saved(&store, &id).unwrap();
		assert_eq!(restored.selected_profile_id, "b");
		let reopened = store.load().unwrap();
		for profile in &original.profiles {
			assert_eq!(reopened.profiles.iter().find(|p| p.id == profile.id), Some(profile));
		}
		assert_eq!(reopened.profile_sources, original.profile_sources);
		assert!(restore_saved(&store, &id).is_err());
		assert_eq!(store.load().unwrap(), reopened);
		assert_eq!(list(&archives).unwrap().len(), 1);
		let mut empty = reopened;
		empty.profiles.clear();
		empty.profile_sources.clear();
		empty.selected_profile_id.clear();
		store.save(empty).unwrap();
		assert_eq!(restore_saved(&store, &id).unwrap().selected_profile_id, "a");
		assert!(load(&archives, &id).is_ok());
		fs::remove_dir_all(&root).unwrap();
	}
	#[test]
	fn rejects_invalid_snapshots_before_mutating_document() {
		let snapshot = ArchivedProfile::capture(&fixture(), "a").unwrap();
		let mut future = snapshot.clone();
		future.version = 2;
		let mut wrong_owner = snapshot.clone();
		wrong_owner.sources[0].profile_id = "other".into();
		let mut duplicate = snapshot.clone();
		duplicate.sources.push(duplicate.sources[0].clone());
		for invalid in [future, wrong_owner, duplicate] {
			let mut doc = fixture();
			doc.profiles.clear();
			doc.profile_sources.clear();
			let before = doc.clone();
			assert!(invalid.restore_into(&mut doc).is_err());
			assert_eq!(doc, before);
		}
	}
	#[test]
	fn archive_write_failure_leaves_original_document_intact() {
		let root = std::env::temp_dir().join(format!(
			"unm-archive-failure-{}-{}",
			std::process::id(),
			SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
		));
		fs::write(&root, b"existing file").unwrap();
		let doc = fixture();
		let before = doc.clone();
		let snapshot = ArchivedProfile::capture(&doc, "a").unwrap();
		assert!(store(&root, &snapshot).is_err());
		assert_eq!(doc, before);
		assert_eq!(fs::read(&root).unwrap(), b"existing file");
		fs::remove_file(&root).unwrap();
	}
	#[test]
	fn archive_roundtrip_keeps_snapshot_after_restore_and_rejects_corruption() {
		let root = std::env::temp_dir().join(format!(
			"unm-archive-{}-{}",
			std::process::id(),
			SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
		));
		let snapshot = ArchivedProfile::capture(&fixture(), "a").unwrap();
		let id = store(&root, &snapshot).unwrap();
		let restored = load(&root, &id).unwrap();
		assert_eq!(restored.profile, snapshot.profile);
		assert_eq!(restored.sources, snapshot.sources);
		let mut doc = fixture();
		doc.profiles.clear();
		doc.profile_sources.clear();
		restored.restore_into(&mut doc).unwrap();
		assert!(load(&root, &id).is_ok());
		assert!(load(&root, "../bad").is_err());
		fs::write(root.join(format!("{id}.json")), b"broken").unwrap();
		assert!(load(&root, &id).is_err());
		fs::remove_dir_all(&root).unwrap();
	}
}
