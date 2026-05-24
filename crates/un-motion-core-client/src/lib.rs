use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CoreProfile {
	pub id: String,
	pub name: String,
	#[serde(default)]
	pub note: String,
	#[serde(default)]
	pub icon_path: Option<String>,
	#[serde(default)]
	pub group: String,
	#[serde(default)]
	pub engine: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
	pub running: bool,
	pub health: String,
	pub active_profile_id: String,
	pub frame_count: u64,
	pub packet_count: u64,
	pub updated_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CoreSnapshot {
	pub status: RuntimeStatus,
	pub profiles: Vec<CoreProfile>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CoreEventKind {
	RuntimeStarted,
	RuntimeStopped,
	ActiveProfileChanged,
	Snapshot,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CoreEvent {
	pub sequence: u64,
	pub kind: CoreEventKind,
	pub timestamp_ms: u64,
	pub snapshot: CoreSnapshot,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActiveProfileRequest {
	pub profile_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct StatusResponse {
	status: RuntimeStatus,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct SnapshotResponse {
	snapshot: CoreSnapshot,
}

#[derive(Clone, Debug)]
pub struct CoreApiClient {
	base_url: String,
	http: reqwest::Client,
}

impl Default for CoreApiClient {
	fn default() -> Self {
		Self::new("http://127.0.0.1:39580")
	}
}

impl CoreApiClient {
	pub fn new(base_url: impl Into<String>) -> Self {
		Self {
			base_url: base_url.into().trim_end_matches('/').to_string(),
			http: reqwest::Client::new(),
		}
	}

	pub fn base_url(&self) -> &str {
		&self.base_url
	}

	pub async fn status(&self) -> anyhow::Result<RuntimeStatus> {
		Ok(self.get::<StatusResponse>("/api/status").await?.status)
	}

	pub async fn snapshot(&self) -> anyhow::Result<CoreSnapshot> {
		Ok(self.get::<SnapshotResponse>("/api/profiles").await?.snapshot)
	}

	pub async fn profiles(&self) -> anyhow::Result<Vec<CoreProfile>> {
		Ok(self.snapshot().await?.profiles)
	}

	pub async fn active_profile(&self) -> anyhow::Result<RuntimeStatus> {
		Ok(self.get::<StatusResponse>("/api/profiles/active").await?.status)
	}

	pub async fn set_active_profile(&self, profile_id: impl Into<String>) -> anyhow::Result<RuntimeStatus> {
		let request = ActiveProfileRequest {
			profile_id: profile_id.into(),
		};
		Ok(self.put::<_, StatusResponse>("/api/profiles/active", &request).await?.status)
	}

	pub async fn start_runtime(&self) -> anyhow::Result<RuntimeStatus> {
		Ok(self.post::<StatusResponse>("/api/runtime/start").await?.status)
	}

	pub async fn stop_runtime(&self) -> anyhow::Result<RuntimeStatus> {
		Ok(self.post::<StatusResponse>("/api/runtime/stop").await?.status)
	}

	fn endpoint(&self, path: &str) -> String {
		format!("{}{}", self.base_url, path)
	}

	async fn get<T>(&self, path: &str) -> anyhow::Result<T>
	where
		T: for<'de> Deserialize<'de>,
	{
		self.http
			.get(self.endpoint(path))
			.send()
			.await
			.with_context(|| format!("GET {path} failed"))?
			.error_for_status()
			.with_context(|| format!("GET {path} returned error status"))?
			.json::<T>()
			.await
			.with_context(|| format!("GET {path} response decode failed"))
	}

	async fn post<T>(&self, path: &str) -> anyhow::Result<T>
	where
		T: for<'de> Deserialize<'de>,
	{
		self.http
			.post(self.endpoint(path))
			.send()
			.await
			.with_context(|| format!("POST {path} failed"))?
			.error_for_status()
			.with_context(|| format!("POST {path} returned error status"))?
			.json::<T>()
			.await
			.with_context(|| format!("POST {path} response decode failed"))
	}

	async fn put<B, T>(&self, path: &str, body: &B) -> anyhow::Result<T>
	where
		B: Serialize + ?Sized,
		T: for<'de> Deserialize<'de>,
	{
		self.http
			.put(self.endpoint(path))
			.json(body)
			.send()
			.await
			.with_context(|| format!("PUT {path} failed"))?
			.error_for_status()
			.with_context(|| format!("PUT {path} returned error status"))?
			.json::<T>()
			.await
			.with_context(|| format!("PUT {path} response decode failed"))
	}
}

#[cfg(test)]
mod tests {
	use actix_web::{App, HttpServer, web};
	use un_motion_core::{CoreControlState, configure_api};

	use super::*;

	#[test]
	fn default_client_uses_local_core_port() {
		assert_eq!(CoreApiClient::default().base_url(), "http://127.0.0.1:39580");
	}

	#[actix_web::test]
	async fn client_controls_core_runtime_through_api_contract() {
		let state = CoreControlState::default();
		let server = HttpServer::new(move || App::new().app_data(web::Data::new(state.clone())).configure(configure_api))
			.bind(("127.0.0.1", 0))
			.expect("bind");
		let addr = server.addrs()[0];
		let handle = server.run();
		actix_web::rt::spawn(handle);
		let client = CoreApiClient::new(format!("http://{addr}"));

		assert!(!client.status().await.expect("status").running);
		assert!(client.start_runtime().await.expect("start").running);
		assert!(!client.stop_runtime().await.expect("stop").running);
		assert_eq!(
			client.set_active_profile("default").await.expect("active").active_profile_id,
			"default"
		);
	}
}
