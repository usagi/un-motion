//! Profiles saved by the ordinary isolated Supervisor UI, then reopened.
//! This checks persisted UI values at the Capturer configuration boundary;
//! it does not start a camera, engine, or output transport.
use super::*;

#[test]
fn saved_advanced_ui_profiles_reach_runtime_without_cross_profile_values() {
    let profiles: Vec<CoreProfileDocumentProfile> = [
        include_str!("../tests/fixtures/advanced-ui/default.toml"),
        include_str!("../tests/fixtures/advanced-ui/peer.toml"),
    ].into_iter().map(|raw| toml::from_str(raw).unwrap()).collect();
    for (raw, width, height, ffmpeg) in [
        (include_str!("../tests/fixtures/advanced-ui/default.toml"), 1280, 720, "D:/reviewed/ffmpeg.exe"),
        (include_str!("../tests/fixtures/advanced-ui/peer.toml"), 1281, 721, "D:/profile-1/ffmpeg.exe"),
    ] {
        let profile: CoreProfileDocumentProfile = toml::from_str(raw).unwrap();
        let original = profile.clone();
        let document = CoreProfileDocument {
            selected_profile_id: profile.id.clone(), profiles: profiles.clone(),
            profile_sources: vec![], next_profile_index: 3, next_source_index: 1,
        };
        let config = core_runtime_config_from_document(&document).unwrap();
        let stream = &config.frame_streams[0];
        let resize = stream.input_resize.as_ref().unwrap();
        assert_eq!((resize.width, resize.height, resize.reference), (width, height, height));
        assert_eq!(resize.axis, "height");
        assert!(!resize.preserve_aspect_ratio);
        assert_eq!(resize.pad_color, "000000ff");
        assert_eq!(stream.input_ffmpeg_path.as_deref(), Some(ffmpeg));
        assert_eq!(stream.post_process_component, "none");
        assert!(!stream.media_pipe_post_process.post_process_rules.hold_lost_landmarks);
        assert_eq!(document.profiles, profiles);
        assert_eq!(document.profiles.iter().find(|p| p.id == original.id), Some(&original));
    }
}

#[test]
fn cleared_resize_sizes_follow_input_dimensions_and_selected_axis() {
    let mut profile: CoreProfileDocumentProfile = toml::from_str(include_str!("../tests/fixtures/advanced-ui/peer.toml")).unwrap();
    let pipeline = profile.pipeline_components.as_mut().unwrap();
    pipeline.input_resize_width = None;
    pipeline.input_resize_height = None;
    pipeline.input_resize_reference = None;
    pipeline.input_width = Some(960);
    pipeline.input_height = Some(540);
    for (axis, reference) in [("height", 540), ("width", 960)] {
        profile.pipeline_components.as_mut().unwrap().input_resize_axis = Some(axis.into());
        let stream = core_frame_stream_config(profile.id.clone(), StreamId::new("advanced-review"), profile.runtime_selection.as_ref(), profile.pipeline_components.as_ref());
        let resize = stream.input_resize.unwrap();
        assert_eq!((resize.width, resize.height, resize.reference), (960, 540, reference));
    }
}
