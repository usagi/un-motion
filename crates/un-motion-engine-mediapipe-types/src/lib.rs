use un_motion_mediapipe_native::NativeMediaPipeOutput;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum MediaPipeRawOutput {
	#[default]
	Empty,
	Native(NativeMediaPipeOutput),
}

impl MediaPipeRawOutput {
	pub fn native(&self) -> Option<&NativeMediaPipeOutput> {
		match self {
			Self::Native(output) => Some(output),
			Self::Empty => None,
		}
	}
}

impl From<NativeMediaPipeOutput> for MediaPipeRawOutput {
	fn from(output: NativeMediaPipeOutput) -> Self {
		Self::Native(output)
	}
}
