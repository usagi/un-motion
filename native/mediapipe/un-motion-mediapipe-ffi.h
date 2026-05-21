#pragma once

#include <stdint.h>

#ifdef _WIN32
#define UN_MOTION_MEDIAPIPE_EXPORT __declspec(dllexport)
#else
#define UN_MOTION_MEDIAPIPE_EXPORT __attribute__((visibility("default")))
#endif


#ifdef __cplusplus
extern "C" {
#endif

// MediaPipe Hand Landmarker: 21 normalized landmarks per hand.
#define UN_MOTION_MEDIAPIPE_HAND_LANDMARK_COUNT 21
// Task is configured with num_hands=2 for two-hand capture.
#define UN_MOTION_MEDIAPIPE_MAX_HANDS 2
#define UN_MOTION_MEDIAPIPE_FACE_LANDMARK_COUNT 478
#define UN_MOTION_MEDIAPIPE_MAX_FACE_BLENDSHAPES 64
#define UN_MOTION_MEDIAPIPE_BLENDSHAPE_NAME_BYTES 64
#define UN_MOTION_MEDIAPIPE_MAX_GESTURES 2
#define UN_MOTION_MEDIAPIPE_MAX_GESTURE_CATEGORIES 8
#define UN_MOTION_MEDIAPIPE_GESTURE_NAME_BYTES 64


typedef struct UnMotionMediaPipeLandmark {
  float x;
  float y;
  float z;
  float visibility;
  float presence;
} UnMotionMediaPipeLandmark;

typedef struct UnMotionMediaPipePose {
  UnMotionMediaPipeLandmark landmarks[33];
  UnMotionMediaPipeLandmark world_landmarks[33];
  uint32_t landmark_count;
  uint32_t world_landmark_count;
  float confidence;
  uint8_t segmentation_mask_present;
  uint32_t segmentation_mask_width;
  uint32_t segmentation_mask_height;
  float segmentation_mask_mean;
} UnMotionMediaPipePose;

typedef struct UnMotionMediaPipeHand {
  UnMotionMediaPipeLandmark landmarks[UN_MOTION_MEDIAPIPE_HAND_LANDMARK_COUNT];
  UnMotionMediaPipeLandmark world_landmarks[UN_MOTION_MEDIAPIPE_HAND_LANDMARK_COUNT];
  uint32_t landmark_count;
  uint32_t world_landmark_count;
  float handedness_score;
  // 0 = Left, 1 = Right, 255 = unknown / missing metadata.
  uint8_t handedness_is_right;
  float confidence;
} UnMotionMediaPipeHand;

typedef struct UnMotionMediaPipeHands {
  UnMotionMediaPipeHand hands[UN_MOTION_MEDIAPIPE_MAX_HANDS];
  uint32_t hand_count;
} UnMotionMediaPipeHands;

typedef struct UnMotionMediaPipeFaceBlendshape {
  char name[UN_MOTION_MEDIAPIPE_BLENDSHAPE_NAME_BYTES];
  float score;
} UnMotionMediaPipeFaceBlendshape;

typedef struct UnMotionMediaPipeFace {
  UnMotionMediaPipeLandmark landmarks[UN_MOTION_MEDIAPIPE_FACE_LANDMARK_COUNT];
  uint32_t landmark_count;
  float confidence;
  float matrix[16];
  uint32_t matrix_rows;
  uint32_t matrix_cols;
  UnMotionMediaPipeFaceBlendshape blendshapes[UN_MOTION_MEDIAPIPE_MAX_FACE_BLENDSHAPES];
  uint32_t blendshape_count;
} UnMotionMediaPipeFace;

typedef struct UnMotionMediaPipeGestureCategory {
  char name[UN_MOTION_MEDIAPIPE_GESTURE_NAME_BYTES];
  float score;
} UnMotionMediaPipeGestureCategory;

typedef struct UnMotionMediaPipeGesture {
  UnMotionMediaPipeGestureCategory categories[UN_MOTION_MEDIAPIPE_MAX_GESTURE_CATEGORIES];
  uint32_t category_count;
  uint8_t handedness_is_right;
  float handedness_score;
} UnMotionMediaPipeGesture;

typedef struct UnMotionMediaPipeGestures {
  UnMotionMediaPipeGesture gestures[UN_MOTION_MEDIAPIPE_MAX_GESTURES];
  uint32_t gesture_count;
} UnMotionMediaPipeGestures;

typedef struct UnMotionMediaPipeHolistic {
  UnMotionMediaPipePose pose;
  UnMotionMediaPipeHand left_hand;
  UnMotionMediaPipeHand right_hand;
  UnMotionMediaPipeFace face;
} UnMotionMediaPipeHolistic;

#define UN_MOTION_MEDIAPIPE_RUNNING_MODE_IMAGE 0
#define UN_MOTION_MEDIAPIPE_RUNNING_MODE_VIDEO 1
#define UN_MOTION_MEDIAPIPE_RUNNING_MODE_LIVE_STREAM 2

#define UN_MOTION_MEDIAPIPE_DELEGATE_CPU 0
#define UN_MOTION_MEDIAPIPE_DELEGATE_XNNPACK 1
#define UN_MOTION_MEDIAPIPE_DELEGATE_GPU 2

typedef struct UnMotionMediaPipeOptions {
  uint32_t abi_size;
  uint32_t running_mode;
  uint8_t enable_pose;
  uint8_t enable_hands;
  uint8_t enable_face;
  uint8_t enable_gestures;
  uint8_t enable_holistic;
  uint8_t output_pose_segmentation;
  uint8_t delegate;
  uint32_t delegate_num_threads;
  uint8_t holistic_flow_limiter_enabled;
  uint32_t holistic_flow_limiter_max_in_flight;
  uint32_t holistic_flow_limiter_max_in_queue;
} UnMotionMediaPipeOptions;

UN_MOTION_MEDIAPIPE_EXPORT void* un_motion_mediapipe_create(void);
UN_MOTION_MEDIAPIPE_EXPORT void* un_motion_mediapipe_create_with_options(
    const UnMotionMediaPipeOptions* options);
UN_MOTION_MEDIAPIPE_EXPORT void un_motion_mediapipe_destroy(void* context);

// rgb points to tightly packed RGB888 rows. stride is bytes per row.
// Returns 0 on success. Non-zero means no usable pose or backend failure.
UN_MOTION_MEDIAPIPE_EXPORT int32_t un_motion_mediapipe_process_rgb(
    void* context,
    const uint8_t* rgb,
    uint32_t width,
    uint32_t height,
    uint32_t stride,
    UnMotionMediaPipePose* out_pose);

// Runs pose and/or hand landmarkers on the same frame. Either out pointer may be NULL.
// At least one of out_pose / out_hands must be non-NULL.
// Builds the input image once; when both outputs are requested this is cheaper than
// calling un_motion_mediapipe_process_rgb twice.
//
// Hand model path: UN_MOTION_MEDIAPIPE_HAND_MODEL or default
// models/hand_landmarker.task
// (relative to Bazel runfiles cwd, typically third_party/mediapipe).
//
// Return codes (see UNMotion docs/media-pipe-native-requirements.md):
//   0   requested branches completed (pose may be empty when out_pose set)
//   10  invalid arguments
//   11  missing backend
//   13  pose Detect failed (when out_pose != NULL)
//   20  no pose landmarks (when out_pose != NULL; hands may still be filled)
//   21  insufficient pose landmarks
//   42  face Detect failed (when out_face != NULL; overwrites pose-only rc)
//   52  hand Detect failed (when out_hands != NULL; overwrites pose-only rc)
UN_MOTION_MEDIAPIPE_EXPORT int32_t un_motion_mediapipe_process_rgb_pose_and_hands(
    void* context,
    const uint8_t* rgb,
    uint32_t width,
    uint32_t height,
    uint32_t stride,
    UnMotionMediaPipePose* out_pose,
    UnMotionMediaPipeHands* out_hands);

UN_MOTION_MEDIAPIPE_EXPORT int32_t un_motion_mediapipe_process_rgb_full(
    void* context,
    const uint8_t* rgb,
    uint32_t width,
    uint32_t height,
    uint32_t stride,
    UnMotionMediaPipePose* out_pose,
    UnMotionMediaPipeHands* out_hands,
    UnMotionMediaPipeFace* out_face);

UN_MOTION_MEDIAPIPE_EXPORT int32_t un_motion_mediapipe_process_rgb_everything(
    void* context,
    const uint8_t* rgb,
    uint32_t width,
    uint32_t height,
    uint32_t stride,
    UnMotionMediaPipePose* out_pose,
    UnMotionMediaPipeHands* out_hands,
    UnMotionMediaPipeFace* out_face,
    UnMotionMediaPipeGestures* out_gestures,
    UnMotionMediaPipeHolistic* out_holistic);

UN_MOTION_MEDIAPIPE_EXPORT int32_t un_motion_mediapipe_process_rgb_everything_at(
    void* context,
    const uint8_t* rgb,
    uint32_t width,
    uint32_t height,
    uint32_t stride,
    int64_t timestamp_ms,
    UnMotionMediaPipePose* out_pose,
    UnMotionMediaPipeHands* out_hands,
    UnMotionMediaPipeFace* out_face,
    UnMotionMediaPipeGestures* out_gestures,
    UnMotionMediaPipeHolistic* out_holistic);

// Polls the latest LIVE_STREAM callback results for timestamp_ms or newer.
// Returns 0 when every requested branch has a callback result available.
// Returns 30 when at least one requested branch has not produced a callback result yet.
UN_MOTION_MEDIAPIPE_EXPORT int32_t un_motion_mediapipe_poll_latest_at(
    void* context,
    int64_t timestamp_ms,
    UnMotionMediaPipePose* out_pose,
    UnMotionMediaPipeHands* out_hands,
    UnMotionMediaPipeFace* out_face,
    UnMotionMediaPipeGestures* out_gestures,
    UnMotionMediaPipeHolistic* out_holistic);

// Same as un_motion_mediapipe_poll_latest_at, and also returns the timestamp
// of the coherent latest result set. For multiple branches this is the oldest
// branch timestamp in the returned set.
UN_MOTION_MEDIAPIPE_EXPORT int32_t un_motion_mediapipe_poll_latest_timestamp_at(
    void* context,
    int64_t timestamp_ms,
    UnMotionMediaPipePose* out_pose,
    UnMotionMediaPipeHands* out_hands,
    UnMotionMediaPipeFace* out_face,
    UnMotionMediaPipeGestures* out_gestures,
    UnMotionMediaPipeHolistic* out_holistic,
    int64_t* out_result_timestamp_ms);

typedef struct UnMotionMediaPipeLiveStreamStats {
    uint64_t pose_submit_count;
    uint64_t hands_submit_count;
    uint64_t face_submit_count;
    uint64_t gestures_submit_count;
    uint64_t holistic_submit_count;
    uint64_t pose_submit_error_count;
    uint64_t hands_submit_error_count;
    uint64_t face_submit_error_count;
    uint64_t gestures_submit_error_count;
    uint64_t holistic_submit_error_count;
    uint64_t pose_callback_count;
    uint64_t hands_callback_count;
    uint64_t face_callback_count;
    uint64_t gestures_callback_count;
    uint64_t holistic_callback_count;
    int64_t latest_pose_timestamp_ms;
    int64_t latest_hands_timestamp_ms;
    int64_t latest_face_timestamp_ms;
    int64_t latest_gestures_timestamp_ms;
    int64_t latest_holistic_timestamp_ms;
} UnMotionMediaPipeLiveStreamStats;

UN_MOTION_MEDIAPIPE_EXPORT int32_t un_motion_mediapipe_live_stream_stats(
    void* context,
    UnMotionMediaPipeLiveStreamStats* out_stats);

#ifdef __cplusplus
}
#endif
