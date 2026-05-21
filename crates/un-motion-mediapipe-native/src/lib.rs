use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::Arc;

use anyhow::Context;
use libloading::Library;

pub const POSE_LANDMARK_COUNT: usize = 33;
pub const HAND_LANDMARK_COUNT: usize = 21;
pub const FACE_LANDMARK_COUNT: usize = 478;
pub const MAX_FACE_BLENDSHAPES: usize = 64;
pub const BLENDSHAPE_NAME_BYTES: usize = 64;
pub const MAX_HANDS: usize = 2;
pub const MAX_GESTURES: usize = 2;
pub const MAX_GESTURE_CATEGORIES: usize = 8;
pub const GESTURE_NAME_BYTES: usize = 64;
pub const RUNNING_MODE_IMAGE: u32 = 0;
pub const RUNNING_MODE_VIDEO: u32 = 1;
pub const RUNNING_MODE_LIVE_STREAM: u32 = 2;
pub const DELEGATE_CPU: u8 = 0;
pub const DELEGATE_XNNPACK: u8 = 1;
pub const DELEGATE_GPU: u8 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeMediaPipeOptions {
	pub abi_size: u32,
	pub running_mode: u32,
	pub enable_pose: u8,
	pub enable_hands: u8,
	pub enable_face: u8,
	pub enable_gestures: u8,
	pub enable_holistic: u8,
	pub output_pose_segmentation: u8,
	pub delegate: u8,
	pub delegate_num_threads: u32,
	pub holistic_flow_limiter_enabled: u8,
	pub holistic_flow_limiter_max_in_flight: u32,
	pub holistic_flow_limiter_max_in_queue: u32,
}

impl NativeMediaPipeOptions {
	pub fn desktop_live_stream() -> Self {
		Self {
			running_mode: RUNNING_MODE_LIVE_STREAM,
			..Self::desktop_video()
		}
	}

	pub fn desktop_video() -> Self {
		Self {
			abi_size: std::mem::size_of::<Self>() as u32,
			running_mode: RUNNING_MODE_VIDEO,
			enable_pose: 1,
			enable_hands: 1,
			enable_face: 1,
			enable_gestures: 0,
			enable_holistic: 0,
			output_pose_segmentation: 0,
			delegate: DELEGATE_XNNPACK,
			delegate_num_threads: 2,
			holistic_flow_limiter_enabled: 1,
			holistic_flow_limiter_max_in_flight: 1,
			holistic_flow_limiter_max_in_queue: 1,
		}
	}

	pub fn image_all() -> Self {
		Self {
			running_mode: RUNNING_MODE_IMAGE,
			enable_gestures: 1,
			..Self::desktop_video()
		}
	}
}

impl Default for NativeMediaPipeOptions {
	fn default() -> Self {
		Self::desktop_video()
	}
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NativeLandmark {
	pub x: f32,
	pub y: f32,
	pub z: f32,
	pub visibility: f32,
	pub presence: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativePose {
	pub landmarks: [NativeLandmark; POSE_LANDMARK_COUNT],
	pub world_landmarks: [NativeLandmark; POSE_LANDMARK_COUNT],
	pub landmark_count: u32,
	pub world_landmark_count: u32,
	pub confidence: f32,
	pub segmentation_mask_present: u8,
	pub segmentation_mask_width: u32,
	pub segmentation_mask_height: u32,
	pub segmentation_mask_mean: f32,
}

impl Default for NativePose {
	fn default() -> Self {
		Self {
			landmarks: [NativeLandmark::default(); POSE_LANDMARK_COUNT],
			world_landmarks: [NativeLandmark::default(); POSE_LANDMARK_COUNT],
			landmark_count: 0,
			world_landmark_count: 0,
			confidence: 0.0,
			segmentation_mask_present: 0,
			segmentation_mask_width: 0,
			segmentation_mask_height: 0,
			segmentation_mask_mean: 0.0,
		}
	}
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeHand {
	pub landmarks: [NativeLandmark; HAND_LANDMARK_COUNT],
	pub world_landmarks: [NativeLandmark; HAND_LANDMARK_COUNT],
	pub landmark_count: u32,
	pub world_landmark_count: u32,
	pub handedness_score: f32,
	pub handedness_is_right: u8,
	pub confidence: f32,
}

impl Default for NativeHand {
	fn default() -> Self {
		Self {
			landmarks: [NativeLandmark::default(); HAND_LANDMARK_COUNT],
			world_landmarks: [NativeLandmark::default(); HAND_LANDMARK_COUNT],
			landmark_count: 0,
			world_landmark_count: 0,
			handedness_score: 0.0,
			handedness_is_right: 255,
			confidence: 0.0,
		}
	}
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NativeHands {
	pub hands: [NativeHand; MAX_HANDS],
	pub hand_count: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeFaceBlendshape {
	pub name: [u8; BLENDSHAPE_NAME_BYTES],
	pub score: f32,
}

impl Default for NativeFaceBlendshape {
	fn default() -> Self {
		Self {
			name: [0; BLENDSHAPE_NAME_BYTES],
			score: 0.0,
		}
	}
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeFace {
	pub landmarks: [NativeLandmark; FACE_LANDMARK_COUNT],
	pub landmark_count: u32,
	pub confidence: f32,
	pub matrix: [f32; 16],
	pub matrix_rows: u32,
	pub matrix_cols: u32,
	pub blendshapes: [NativeFaceBlendshape; MAX_FACE_BLENDSHAPES],
	pub blendshape_count: u32,
}

impl Default for NativeFace {
	fn default() -> Self {
		Self {
			landmarks: [NativeLandmark::default(); FACE_LANDMARK_COUNT],
			landmark_count: 0,
			confidence: 0.0,
			matrix: [0.0; 16],
			matrix_rows: 0,
			matrix_cols: 0,
			blendshapes: [NativeFaceBlendshape::default(); MAX_FACE_BLENDSHAPES],
			blendshape_count: 0,
		}
	}
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeGestureCategory {
	pub name: [u8; GESTURE_NAME_BYTES],
	pub score: f32,
}

impl Default for NativeGestureCategory {
	fn default() -> Self {
		Self {
			name: [0; GESTURE_NAME_BYTES],
			score: 0.0,
		}
	}
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeGesture {
	pub categories: [NativeGestureCategory; MAX_GESTURE_CATEGORIES],
	pub category_count: u32,
	pub handedness_is_right: u8,
	pub handedness_score: f32,
}

impl Default for NativeGesture {
	fn default() -> Self {
		Self {
			categories: [NativeGestureCategory::default(); MAX_GESTURE_CATEGORIES],
			category_count: 0,
			handedness_is_right: 255,
			handedness_score: 0.0,
		}
	}
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NativeGestures {
	pub gestures: [NativeGesture; MAX_GESTURES],
	pub gesture_count: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NativeHolistic {
	pub pose: NativePose,
	pub left_hand: NativeHand,
	pub right_hand: NativeHand,
	pub face: NativeFace,
}

#[derive(Clone, Copy, Debug)]
pub struct RgbImageRef<'a> {
	pub bytes: &'a [u8],
	pub width: u32,
	pub height: u32,
	pub stride: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NativeMediaPipeOutput {
	pub return_code: i32,
	pub result_timestamp_ms: Option<i64>,
	pub pose: NativePose,
	pub hands: NativeHands,
	pub face: NativeFace,
	pub gestures: NativeGestures,
	pub holistic: NativeHolistic,
}

#[rustfmt::skip]
type CreateFn = unsafe extern "C" fn() -> *mut c_void;
#[rustfmt::skip]
type CreateWithOptionsFn = unsafe extern "C" fn(*const NativeMediaPipeOptions) -> *mut c_void;
#[rustfmt::skip]
type DestroyFn = unsafe extern "C" fn(*mut c_void);
#[rustfmt::skip]
type ProcessPoseFn = unsafe extern "C" fn(*mut c_void, *const u8, u32, u32, u32, *mut NativePose) -> i32;
#[rustfmt::skip]
type ProcessPoseHandsFn = unsafe extern "C" fn(*mut c_void, *const u8, u32, u32, u32, *mut NativePose, *mut NativeHands) -> i32;
#[rustfmt::skip]
type ProcessFullFn = unsafe extern "C" fn(*mut c_void, *const u8, u32, u32, u32, *mut NativePose, *mut NativeHands, *mut NativeFace) -> i32;
#[rustfmt::skip]
type ProcessEverythingFn = unsafe extern "C" fn(*mut c_void, *const u8, u32, u32, u32, *mut NativePose, *mut NativeHands, *mut NativeFace, *mut NativeGestures, *mut NativeHolistic) -> i32;
#[rustfmt::skip]
type ProcessEverythingAtFn = unsafe extern "C" fn(*mut c_void, *const u8, u32, u32, u32, i64, *mut NativePose, *mut NativeHands, *mut NativeFace, *mut NativeGestures, *mut NativeHolistic) -> i32;
#[rustfmt::skip]
type PollLatestAtFn = unsafe extern "C" fn(*mut c_void, i64, *mut NativePose, *mut NativeHands, *mut NativeFace, *mut NativeGestures, *mut NativeHolistic) -> i32;
#[rustfmt::skip]
type PollLatestTimestampAtFn = unsafe extern "C" fn(*mut c_void, i64, *mut NativePose, *mut NativeHands, *mut NativeFace, *mut NativeGestures, *mut NativeHolistic, *mut i64) -> i32;
#[rustfmt::skip]
type LiveStreamStatsFn = unsafe extern "C" fn(*mut c_void, *mut NativeLiveStreamStats) -> i32;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NativeLiveStreamStats {
	pub pose_submit_count: u64,
	pub hands_submit_count: u64,
	pub face_submit_count: u64,
	pub gestures_submit_count: u64,
	pub holistic_submit_count: u64,
	pub pose_submit_error_count: u64,
	pub hands_submit_error_count: u64,
	pub face_submit_error_count: u64,
	pub gestures_submit_error_count: u64,
	pub holistic_submit_error_count: u64,
	pub pose_callback_count: u64,
	pub hands_callback_count: u64,
	pub face_callback_count: u64,
	pub gestures_callback_count: u64,
	pub holistic_callback_count: u64,
	pub latest_pose_timestamp_ms: i64,
	pub latest_hands_timestamp_ms: i64,
	pub latest_face_timestamp_ms: i64,
	pub latest_gestures_timestamp_ms: i64,
	pub latest_holistic_timestamp_ms: i64,
}

struct NativeMediaPipeApi {
	_library: Library,
	create: CreateFn,
	create_with_options: Option<CreateWithOptionsFn>,
	destroy: DestroyFn,
	process_pose: Option<ProcessPoseFn>,
	process_pose_hands: Option<ProcessPoseHandsFn>,
	process_full: Option<ProcessFullFn>,
	process_everything: Option<ProcessEverythingFn>,
	process_everything_at: Option<ProcessEverythingAtFn>,
	poll_latest_at: Option<PollLatestAtFn>,
	poll_latest_timestamp_at: Option<PollLatestTimestampAtFn>,
	live_stream_stats: Option<LiveStreamStatsFn>,
}

pub struct NativeMediaPipeRuntime {
	api: Arc<NativeMediaPipeApi>,
	handle: NonNull<c_void>,
	options: NativeMediaPipeOptions,
	running_mode: u32,
	last_timestamp_ms: i64,
	last_live_stream_output_timestamp_ms: i64,
}

unsafe impl Send for NativeMediaPipeRuntime {}

impl NativeMediaPipeRuntime {
	pub fn open_default() -> anyhow::Result<Self> {
		Self::open_default_with_options(NativeMediaPipeOptions::default())
	}

	pub fn open_default_with_options(options: NativeMediaPipeOptions) -> anyhow::Result<Self> {
		if let Some(path) = std::env::var_os("UN_MOTION_MEDIAPIPE_DLL").map(PathBuf::from) {
			return Self::open_with_options(path, options);
		}

		for candidate in default_dll_candidates() {
			if candidate.exists() {
				return Self::open_with_options(candidate, options);
			}
		}

		anyhow::bail!("MediaPipe C++ backend DLL not found; set UN_MOTION_MEDIAPIPE_DLL or place native/mediapipe/un-motion-mediapipe.dll")
	}

	pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
		Self::open_with_options(path, NativeMediaPipeOptions::default())
	}

	pub fn open_with_options(path: impl AsRef<Path>, options: NativeMediaPipeOptions) -> anyhow::Result<Self> {
		let library = unsafe { Library::new(path.as_ref()) }
			.with_context(|| format!("failed to load MediaPipe C++ backend DLL: {}", path.as_ref().display()))?;
		let create = unsafe { *library.get::<CreateFn>(b"un_motion_mediapipe_create\0")? };
		let create_with_options = unsafe {
			library
				.get::<CreateWithOptionsFn>(b"un_motion_mediapipe_create_with_options\0")
				.ok()
				.map(|symbol| *symbol)
		};
		let destroy = unsafe { *library.get::<DestroyFn>(b"un_motion_mediapipe_destroy\0")? };
		let process_pose = unsafe {
			library
				.get::<ProcessPoseFn>(b"un_motion_mediapipe_process_rgb\0")
				.ok()
				.map(|symbol| *symbol)
		};
		let process_pose_hands = unsafe {
			library
				.get::<ProcessPoseHandsFn>(b"un_motion_mediapipe_process_rgb_pose_and_hands\0")
				.ok()
				.map(|symbol| *symbol)
		};
		let process_full = unsafe {
			library
				.get::<ProcessFullFn>(b"un_motion_mediapipe_process_rgb_full\0")
				.ok()
				.map(|symbol| *symbol)
		};
		let process_everything = unsafe {
			library
				.get::<ProcessEverythingFn>(b"un_motion_mediapipe_process_rgb_everything\0")
				.ok()
				.map(|symbol| *symbol)
		};
		let process_everything_at = unsafe {
			library
				.get::<ProcessEverythingAtFn>(b"un_motion_mediapipe_process_rgb_everything_at\0")
				.ok()
				.map(|symbol| *symbol)
		};
		let poll_latest_at = unsafe {
			library
				.get::<PollLatestAtFn>(b"un_motion_mediapipe_poll_latest_at\0")
				.ok()
				.map(|symbol| *symbol)
		};
		let poll_latest_timestamp_at = unsafe {
			library
				.get::<PollLatestTimestampAtFn>(b"un_motion_mediapipe_poll_latest_timestamp_at\0")
				.ok()
				.map(|symbol| *symbol)
		};
		let live_stream_stats = unsafe {
			library
				.get::<LiveStreamStatsFn>(b"un_motion_mediapipe_live_stream_stats\0")
				.ok()
				.map(|symbol| *symbol)
		};
		if process_pose.is_none() && process_pose_hands.is_none() && process_full.is_none() && process_everything.is_none() {
			anyhow::bail!("MediaPipe C++ backend DLL exposes no supported un_motion_mediapipe_process_rgb* symbol");
		}
		let api = Arc::new(NativeMediaPipeApi {
			_library: library,
			create,
			create_with_options,
			destroy,
			process_pose,
			process_pose_hands,
			process_full,
			process_everything,
			process_everything_at,
			poll_latest_at,
			poll_latest_timestamp_at,
			live_stream_stats,
		});
		let raw = unsafe {
			if let Some(create_with_options) = api.create_with_options {
				create_with_options(&options)
			} else {
				(api.create)()
			}
		};
		let handle = NonNull::new(raw).context("MediaPipe C++ backend returned a null context")?;
		Ok(Self {
			api,
			handle,
			options,
			running_mode: options.running_mode,
			last_timestamp_ms: -1,
			last_live_stream_output_timestamp_ms: -1,
		})
	}

	pub fn process_rgb(&mut self, image: RgbImageRef<'_>) -> anyhow::Result<NativeMediaPipeOutput> {
		self.process_rgb_with_options(image, false)
	}

	pub fn process_rgb_at(&mut self, image: RgbImageRef<'_>, timestamp_ms: i64) -> anyhow::Result<NativeMediaPipeOutput> {
		self.process_rgb_at_with_options(image, timestamp_ms, false)
	}

	pub fn process_rgb_everything(&mut self, image: RgbImageRef<'_>) -> anyhow::Result<NativeMediaPipeOutput> {
		self.process_rgb_with_options(image, true)
	}

	pub fn process_rgb_everything_at(&mut self, image: RgbImageRef<'_>, timestamp_ms: i64) -> anyhow::Result<NativeMediaPipeOutput> {
		self.process_rgb_at_with_options(image, timestamp_ms, true)
	}

	pub fn poll_latest(&mut self, include_gestures: bool) -> anyhow::Result<NativeMediaPipeOutput> {
		let mut pose = NativePose::default();
		let mut hands = NativeHands::default();
		let mut face = NativeFace::default();
		let mut gestures = NativeGestures::default();
		let mut holistic = NativeHolistic::default();
		let min_result_timestamp_ms = self.last_live_stream_output_timestamp_ms.saturating_add(1);
		let (return_code, result_timestamp_ms) = self.poll_latest_at(
			min_result_timestamp_ms,
			include_gestures,
			0,
			&mut pose,
			&mut hands,
			&mut face,
			&mut gestures,
			&mut holistic,
		)?;
		if return_code != 30 {
			if let Some(latest_timestamp_ms) = result_timestamp_ms {
				self.last_live_stream_output_timestamp_ms = latest_timestamp_ms;
			}
		}
		Ok(NativeMediaPipeOutput {
			return_code,
			result_timestamp_ms,
			pose,
			hands,
			face,
			gestures,
			holistic,
		})
	}

	pub fn live_stream_stats(&mut self) -> Option<NativeLiveStreamStats> {
		let live_stream_stats = self.api.live_stream_stats?;
		let mut stats = NativeLiveStreamStats::default();
		let return_code = unsafe { live_stream_stats(self.handle.as_ptr(), &mut stats) };
		(return_code == 0).then_some(stats)
	}

	fn process_rgb_with_options(&mut self, image: RgbImageRef<'_>, include_gestures: bool) -> anyhow::Result<NativeMediaPipeOutput> {
		self.process_rgb_at_with_options(image, 0, include_gestures)
	}

	fn process_rgb_at_with_options(
		&mut self,
		image: RgbImageRef<'_>,
		mut timestamp_ms: i64,
		include_gestures: bool,
	) -> anyhow::Result<NativeMediaPipeOutput> {
		if image.bytes.is_empty() || image.width == 0 || image.height == 0 || image.stride == 0 {
			anyhow::bail!("RGB image must have non-empty bytes and non-zero dimensions");
		}
		if timestamp_ms <= self.last_timestamp_ms {
			timestamp_ms = self.last_timestamp_ms.saturating_add(1);
		}
		self.last_timestamp_ms = timestamp_ms;

		let mut pose = NativePose::default();
		let mut hands = NativeHands::default();
		let mut face = NativeFace::default();
		let mut gestures = NativeGestures::default();
		let mut holistic = NativeHolistic::default();
		let using_holistic = self.options.enable_holistic != 0;
		let pose_ptr = if self.options.enable_pose != 0 && !using_holistic {
			&mut pose
		} else {
			std::ptr::null_mut()
		};
		let hands_ptr = if self.options.enable_hands != 0 && !using_holistic {
			&mut hands
		} else {
			std::ptr::null_mut()
		};
		let face_ptr = if self.options.enable_face != 0 && !using_holistic {
			&mut face
		} else {
			std::ptr::null_mut()
		};
		let gestures_ptr = if include_gestures && self.options.enable_gestures != 0 {
			&mut gestures
		} else {
			std::ptr::null_mut()
		};
		let holistic_ptr = if self.options.enable_holistic != 0 {
			&mut holistic
		} else {
			std::ptr::null_mut()
		};
		let return_code = unsafe {
			if let Some(process_everything_at) = self.api.process_everything_at {
				process_everything_at(
					self.handle.as_ptr(),
					image.bytes.as_ptr(),
					image.width,
					image.height,
					image.stride,
					timestamp_ms,
					pose_ptr,
					hands_ptr,
					face_ptr,
					gestures_ptr,
					holistic_ptr,
				)
			} else if self.options.enable_holistic != 0 {
				anyhow::bail!("MediaPipe C++ backend DLL does not expose holistic-capable process symbol")
			} else if include_gestures {
				if let Some(process_everything) = self.api.process_everything {
					process_everything(
						self.handle.as_ptr(),
						image.bytes.as_ptr(),
						image.width,
						image.height,
						image.stride,
						pose_ptr,
						hands_ptr,
						face_ptr,
						gestures_ptr,
						holistic_ptr,
					)
				} else {
					self.api.process_full.context("missing full process symbol")?(
						self.handle.as_ptr(),
						image.bytes.as_ptr(),
						image.width,
						image.height,
						image.stride,
						pose_ptr,
						hands_ptr,
						face_ptr,
					)
				}
			} else if let Some(process_full) = self.api.process_full {
				process_full(
					self.handle.as_ptr(),
					image.bytes.as_ptr(),
					image.width,
					image.height,
					image.stride,
					pose_ptr,
					hands_ptr,
					face_ptr,
				)
			} else if let Some(process_everything) = self.api.process_everything {
				process_everything(
					self.handle.as_ptr(),
					image.bytes.as_ptr(),
					image.width,
					image.height,
					image.stride,
					pose_ptr,
					hands_ptr,
					face_ptr,
					std::ptr::null_mut(),
					holistic_ptr,
				)
			} else if let Some(process_pose_hands) = self.api.process_pose_hands {
				process_pose_hands(
					self.handle.as_ptr(),
					image.bytes.as_ptr(),
					image.width,
					image.height,
					image.stride,
					pose_ptr,
					hands_ptr,
				)
			} else {
				self.api.process_pose.context("missing pose process symbol")?(
					self.handle.as_ptr(),
					image.bytes.as_ptr(),
					image.width,
					image.height,
					image.stride,
					pose_ptr,
				)
			}
		};

		if self.running_mode == RUNNING_MODE_LIVE_STREAM {
			return Ok(NativeMediaPipeOutput {
				return_code,
				result_timestamp_ms: None,
				pose,
				hands,
				face,
				gestures,
				holistic,
			});
		}

		Ok(NativeMediaPipeOutput {
			return_code,
			result_timestamp_ms: None,
			pose,
			hands,
			face,
			gestures,
			holistic,
		})
	}

	fn poll_latest_at(
		&mut self,
		timestamp_ms: i64,
		include_gestures: bool,
		submit_return_code: i32,
		pose: &mut NativePose,
		hands: &mut NativeHands,
		face: &mut NativeFace,
		gestures: &mut NativeGestures,
		holistic: &mut NativeHolistic,
	) -> anyhow::Result<(i32, Option<i64>)> {
		if submit_return_code != 0 {
			return Ok((submit_return_code, None));
		}
		let using_holistic = self.options.enable_holistic != 0;
		let pose_ptr = if self.options.enable_pose != 0 && !using_holistic {
			pose
		} else {
			std::ptr::null_mut()
		};
		let hands_ptr = if self.options.enable_hands != 0 && !using_holistic {
			hands
		} else {
			std::ptr::null_mut()
		};
		let face_ptr = if self.options.enable_face != 0 && !using_holistic {
			face
		} else {
			std::ptr::null_mut()
		};
		let gestures_ptr = if include_gestures && self.options.enable_gestures != 0 {
			gestures
		} else {
			std::ptr::null_mut()
		};
		let holistic_ptr = if self.options.enable_holistic != 0 {
			holistic
		} else {
			std::ptr::null_mut()
		};
		if let Some(poll_latest_timestamp_at) = self.api.poll_latest_timestamp_at {
			let mut latest_timestamp_ms = -1_i64;
			let latest_return_code = unsafe {
				poll_latest_timestamp_at(
					self.handle.as_ptr(),
					timestamp_ms,
					pose_ptr,
					hands_ptr,
					face_ptr,
					gestures_ptr,
					holistic_ptr,
					&mut latest_timestamp_ms,
				)
			};
			let result_timestamp_ms = (latest_return_code != 30 && latest_timestamp_ms >= 0).then_some(latest_timestamp_ms);
			return Ok((latest_return_code, result_timestamp_ms));
		}
		let Some(poll_latest_at) = self.api.poll_latest_at else {
			return Ok((submit_return_code, None));
		};
		let latest_return_code = unsafe {
			poll_latest_at(
				self.handle.as_ptr(),
				timestamp_ms,
				pose_ptr,
				hands_ptr,
				face_ptr,
				gestures_ptr,
				holistic_ptr,
			)
		};
		Ok((latest_return_code, None))
	}
}

impl Drop for NativeMediaPipeRuntime {
	fn drop(&mut self) {
		unsafe {
			(self.api.destroy)(self.handle.as_ptr());
		}
	}
}

pub fn default_dll_candidates() -> [PathBuf; 2] {
	[
		PathBuf::from("native/mediapipe/un-motion-mediapipe.dll"),
		PathBuf::from("un-motion-mediapipe.dll"),
	]
}

pub fn resolve_media_pipe_root(input: &Path) -> anyhow::Result<PathBuf> {
	if input.join("models/pose_landmarker_lite.task").exists() {
		return Ok(input.to_path_buf());
	}
	if input.file_name().is_some_and(|name| name == "mediapipe") {
		if let Some(root) = input.parent().and_then(Path::parent) {
			if root.join("models/pose_landmarker_lite.task").exists() {
				return Ok(root.to_path_buf());
			}
		}
	}
	anyhow::bail!(
		"failed to locate media-pipe models from {}; pass media-pipe root or native/mediapipe directory",
		input.display()
	)
}

pub fn resolve_native_dir(input: &Path, media_pipe_root: &Path) -> PathBuf {
	let direct_dll = input.join("un-motion-mediapipe.dll");
	if direct_dll.exists() {
		input.to_path_buf()
	} else {
		media_pipe_root.join("native/mediapipe")
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn resolve_native_dir_prefers_direct_dll_directory() {
		let input = Path::new("native/mediapipe");
		let root = Path::new(".");
		let resolved = resolve_native_dir(input, root);
		assert!(resolved.ends_with("native/mediapipe"));
	}

	#[test]
	fn native_struct_sizes_are_stable_enough_for_ffi() {
		assert_eq!(std::mem::size_of::<NativeLandmark>(), 20);
		assert_eq!(std::mem::size_of::<NativePose>(), 20 * POSE_LANDMARK_COUNT * 2 + 28);
		assert_eq!(std::mem::size_of::<NativeHand>(), 20 * HAND_LANDMARK_COUNT * 2 + 20);
		assert_eq!(std::mem::size_of::<NativeFaceBlendshape>(), BLENDSHAPE_NAME_BYTES + 4);
		assert_eq!(std::mem::size_of::<NativeGestureCategory>(), GESTURE_NAME_BYTES + 4);
	}
}
