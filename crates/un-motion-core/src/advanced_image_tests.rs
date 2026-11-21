use super::*;
use crate::runtime_host::core_runtime_config_from_document;
use un_motion_profile_schema::{CoreProfileDocument, CoreProfileDocumentProfile};

#[test]
fn saved_advanced_profile_produces_expected_image_pixels() {
	let path = std::env::temp_dir().join(format!(
		"unm-advanced-image-{}-{}.png",
		std::process::id(),
		std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap()
			.as_nanos()
	));
	struct Cleanup(PathBuf);
	impl Drop for Cleanup {
		fn drop(&mut self) {
			let _ = std::fs::remove_file(&self.0);
		}
	}
	let _cleanup = Cleanup(path.clone());
	image::RgbaImage::from_pixel(4, 2, image::Rgba([240u8, 30, 10, 255]))
		.save(&path)
		.unwrap();
	let profile: CoreProfileDocumentProfile = toml::from_str(include_str!("../tests/fixtures/advanced-ui/default.toml")).unwrap();
	let mut doc = CoreProfileDocument {
		selected_profile_id: profile.id.clone(),
		profiles: vec![profile],
		..Default::default()
	};
	// Saved normal-UI dimensions and disabled aspect preservation must reach the actual pixels.
	let render = |doc: &CoreProfileDocument| {
		let config = core_runtime_config_from_document(doc).unwrap();
		let input = file_image_input_config(&config.frame_streams[0], path.clone()).unwrap();
		FileImageInputSource::open(input).unwrap().next_image_frame().unwrap().unwrap()
	};
	let frame = render(&doc);
	assert_eq!((frame.width, frame.height, frame.stride_bytes), (1280, 720, 3840));
	assert!(frame.data.chunks_exact(3).all(|p| p == [240, 30, 10]));

	// Exercise the same deserialized profile with aspect preservation and custom padding.
	// This is a synthetic follow-up edit, not another UI-save claim.
	let pipeline = doc.profiles[0].pipeline_components.as_mut().unwrap();
	pipeline.input_resize_preserve_aspect = Some(true);
	pipeline.input_resize_width = Some(8);
	pipeline.input_resize_height = Some(8);
	pipeline.input_resize_reference = Some(8);
	pipeline.input_resize_pad_color = Some("010203ff".into());
	for axis in ["width", "height"] {
		doc.profiles[0].pipeline_components.as_mut().unwrap().input_resize_axis = Some(axis.into());
		let frame = render(&doc);
		assert_eq!((frame.width, frame.height, frame.stride_bytes), (8, 8, 24));
		for y in 0..8 {
			for x in 0..8 {
				let offset = (y * 8 + x) * 3;
				let expected = if (2..6).contains(&y) { [240, 30, 10] } else { [1, 2, 3] };
				assert_eq!(&frame.data[offset..offset + 3], &expected, "axis={axis}, pixel=({x},{y})");
			}
		}
	}
	doc.profiles[0].pipeline_components.as_mut().unwrap().input_resize_enabled = Some(false);
	let frame = render(&doc);
	assert_eq!((frame.width, frame.height, frame.stride_bytes), (4, 2, 12));
	assert!(frame.data.chunks_exact(3).all(|p| p == [240, 30, 10]));
}
