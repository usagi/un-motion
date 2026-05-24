#include "un-motion-mediapipe-ffi.h"

#include <algorithm>
#include <cctype>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <limits>
#include <memory>
#include <mutex>
#include <string>
#include <vector>

#if defined(_WIN32)
#include <stdlib.h>
#endif

#include "mediapipe/framework/formats/image.h"
#include "mediapipe/framework/formats/image_frame.h"
#include "mediapipe/framework/formats/image_format.pb.h"
#include "mediapipe/tasks/cc/core/base_options.h"
#include "mediapipe/tasks/cc/core/host_environment.h"
#include "mediapipe/tasks/cc/vision/core/running_mode.h"
#include "mediapipe/tasks/cc/vision/face_landmarker/face_landmarker.h"
#include "mediapipe/tasks/cc/vision/face_landmarker/face_landmarker_result.h"
#include "mediapipe/tasks/cc/vision/gesture_recognizer/gesture_recognizer.h"
#include "mediapipe/tasks/cc/vision/gesture_recognizer/gesture_recognizer_result.h"
#include "mediapipe/tasks/cc/vision/hand_landmarker/hand_landmarker.h"
#include "mediapipe/tasks/cc/vision/hand_landmarker/hand_landmarker_result.h"
#include "mediapipe/tasks/cc/vision/holistic_landmarker/holistic_landmarker.h"
#include "mediapipe/tasks/cc/vision/holistic_landmarker/holistic_landmarker_result.h"
#include "mediapipe/tasks/cc/vision/pose_landmarker/pose_landmarker.h"
#include "mediapipe/tasks/cc/vision/pose_landmarker/pose_landmarker_result.h"

namespace pose = mediapipe::tasks::vision::pose_landmarker;
namespace hand = mediapipe::tasks::vision::hand_landmarker;
namespace face = mediapipe::tasks::vision::face_landmarker;
namespace gesture = mediapipe::tasks::vision::gesture_recognizer;
namespace holistic = mediapipe::tasks::vision::holistic_landmarker;
namespace vision_core = mediapipe::tasks::vision::core;

namespace {

constexpr const char* kDefaultPoseModelPath = "models/pose_landmarker_lite.task";
constexpr const char* kDefaultHandModelPath = "models/hand_landmarker.task";
constexpr const char* kDefaultFaceModelPath = "models/face_landmarker.task";
constexpr const char* kDefaultGestureModelPath = "models/gesture_recognizer.task";
constexpr const char* kDefaultHolisticModelPath = "models/holistic_landmarker.task";
constexpr size_t kLiveStreamQueueCapacity = 32;

struct QueuedPoseResult {
  UnMotionMediaPipePose value = {};
  int32_t error = 20;
  int64_t timestamp_ms = -1;
};

struct QueuedHandsResult {
  UnMotionMediaPipeHands value = {};
  int32_t error = 0;
  int64_t timestamp_ms = -1;
};

struct QueuedFaceResult {
  UnMotionMediaPipeFace value = {};
  int32_t error = 0;
  int64_t timestamp_ms = -1;
};

struct QueuedGesturesResult {
  UnMotionMediaPipeGestures value = {};
  int32_t error = 0;
  int64_t timestamp_ms = -1;
};

struct QueuedHolisticResult {
  UnMotionMediaPipeHolistic value = {};
  int32_t error = 0;
  int64_t timestamp_ms = -1;
};

struct UnMotionMediaPipeContext {
  std::unique_ptr<pose::PoseLandmarker> pose_landmarker;
  std::unique_ptr<hand::HandLandmarker> hand_landmarker;
  std::unique_ptr<face::FaceLandmarker> face_landmarker;
  std::unique_ptr<gesture::GestureRecognizer> gesture_recognizer;
  std::unique_ptr<holistic::HolisticLandmarker> holistic_landmarker;
  int64_t next_timestamp_ms = 0;
  vision_core::RunningMode running_mode = vision_core::RunningMode::VIDEO;
  std::mutex latest_mutex;
  UnMotionMediaPipePose latest_pose = {};
  UnMotionMediaPipeHands latest_hands = {};
  UnMotionMediaPipeFace latest_face = {};
  UnMotionMediaPipeGestures latest_gestures = {};
  UnMotionMediaPipeHolistic latest_holistic = {};
  std::deque<QueuedPoseResult> queued_pose;
  std::deque<QueuedHandsResult> queued_hands;
  std::deque<QueuedFaceResult> queued_face;
  std::deque<QueuedGesturesResult> queued_gestures;
  std::deque<QueuedHolisticResult> queued_holistic;
  int32_t latest_pose_error = 20;
  int32_t latest_hands_error = 0;
  int32_t latest_face_error = 0;
  int32_t latest_gestures_error = 0;
  int32_t latest_holistic_error = 0;
  int64_t latest_pose_timestamp_ms = -1;
  int64_t latest_hands_timestamp_ms = -1;
  int64_t latest_face_timestamp_ms = -1;
  int64_t latest_gestures_timestamp_ms = -1;
  int64_t latest_holistic_timestamp_ms = -1;
  uint64_t pose_submit_count = 0;
  uint64_t hands_submit_count = 0;
  uint64_t face_submit_count = 0;
  uint64_t gestures_submit_count = 0;
  uint64_t holistic_submit_count = 0;
  uint64_t pose_submit_error_count = 0;
  uint64_t hands_submit_error_count = 0;
  uint64_t face_submit_error_count = 0;
  uint64_t gestures_submit_error_count = 0;
  uint64_t holistic_submit_error_count = 0;
  uint64_t pose_callback_count = 0;
  uint64_t hands_callback_count = 0;
  uint64_t face_callback_count = 0;
  uint64_t gestures_callback_count = 0;
  uint64_t holistic_callback_count = 0;
  bool latest_pose_ready = false;
  bool latest_hands_ready = false;
  bool latest_face_ready = false;
  bool latest_gestures_ready = false;
  bool latest_holistic_ready = false;
};

template <typename Queue, typename Item>
void push_bounded(Queue& queue, Item item) {
  queue.push_back(item);
  while (queue.size() > kLiveStreamQueueCapacity) {
    queue.pop_front();
  }
}

template <typename Queue>
bool pop_first_at_or_after(Queue& queue, int64_t timestamp_ms, typename Queue::value_type* out) {
  while (!queue.empty() && queue.front().timestamp_ms < timestamp_ms) {
    queue.pop_front();
  }
  if (queue.empty()) {
    return false;
  }
  *out = queue.front();
  queue.pop_front();
  return true;
}

const char* pose_model_path() {
  const char* env = std::getenv("UN_MOTION_MEDIAPIPE_MODEL");
  if (env != nullptr && env[0] != '\0') {
    return env;
  }
  return kDefaultPoseModelPath;
}

const char* hand_model_path() {
  const char* env = std::getenv("UN_MOTION_MEDIAPIPE_HAND_MODEL");
  if (env != nullptr && env[0] != '\0') {
    return env;
  }
  return kDefaultHandModelPath;
}

const char* face_model_path() {
  const char* env = std::getenv("UN_MOTION_MEDIAPIPE_FACE_MODEL");
  if (env != nullptr && env[0] != '\0') {
    return env;
  }
  return kDefaultFaceModelPath;
}

const char* gesture_model_path() {
  const char* env = std::getenv("UN_MOTION_MEDIAPIPE_GESTURE_MODEL");
  if (env != nullptr && env[0] != '\0') {
    return env;
  }
  return kDefaultGestureModelPath;
}

const char* holistic_model_path() {
  const char* env = std::getenv("UN_MOTION_MEDIAPIPE_HOLISTIC_MODEL");
  if (env != nullptr && env[0] != '\0') {
    return env;
  }
  return kDefaultHolisticModelPath;
}

float score_for(const mediapipe::tasks::components::containers::NormalizedLandmark& landmark) {
  if (landmark.visibility.has_value() && landmark.presence.has_value()) {
    return std::max(*landmark.visibility, *landmark.presence);
  }
  if (landmark.visibility.has_value()) {
    return *landmark.visibility;
  }
  if (landmark.presence.has_value()) {
    return *landmark.presence;
  }
  return 1.0f;
}

float score_for(const mediapipe::tasks::components::containers::Landmark& landmark) {
  if (landmark.visibility.has_value() && landmark.presence.has_value()) {
    return std::max(*landmark.visibility, *landmark.presence);
  }
  if (landmark.visibility.has_value()) {
    return *landmark.visibility;
  }
  if (landmark.presence.has_value()) {
    return *landmark.presence;
  }
  return 1.0f;
}

void copy_world_landmarks(
    const mediapipe::tasks::components::containers::Landmarks& src_list,
    UnMotionMediaPipeLandmark* dst_landmarks,
    uint32_t dst_capacity,
    uint32_t* out_count) {
  const uint32_t count =
      std::min<uint32_t>(static_cast<uint32_t>(src_list.landmarks.size()), dst_capacity);
  *out_count = count;
  for (uint32_t i = 0; i < count; ++i) {
    const auto& src = src_list.landmarks[i];
    UnMotionMediaPipeLandmark& dst = dst_landmarks[i];
    dst.x = src.x;
    dst.y = src.y;
    dst.z = src.z;
    dst.visibility = src.visibility.value_or(score_for(src));
    dst.presence = src.presence.value_or(score_for(src));
  }
}

void copy_world_landmarks_proto(
    const mediapipe::LandmarkList& src_list,
    UnMotionMediaPipeLandmark* dst_landmarks,
    uint32_t dst_capacity,
    uint32_t* out_count) {
  const uint32_t count =
      std::min<uint32_t>(static_cast<uint32_t>(src_list.landmark_size()), dst_capacity);
  *out_count = count;
  for (uint32_t i = 0; i < count; ++i) {
    const auto& src = src_list.landmark(i);
    UnMotionMediaPipeLandmark& dst = dst_landmarks[i];
    dst.x = src.x();
    dst.y = src.y();
    dst.z = src.z();
    dst.visibility = src.has_visibility() ? src.visibility() : 1.0f;
    dst.presence = src.has_presence() ? src.presence() : dst.visibility;
  }
}

void copy_normalized_landmarks(
    const mediapipe::tasks::components::containers::NormalizedLandmarks& src_list,
    UnMotionMediaPipeLandmark* dst_landmarks,
    uint32_t dst_capacity,
    uint32_t* out_count,
    float* out_confidence) {
  const uint32_t count =
      std::min<uint32_t>(static_cast<uint32_t>(src_list.landmarks.size()), dst_capacity);
  *out_count = count;
  float confidence_sum = 0.0f;
  for (uint32_t i = 0; i < count; ++i) {
    const auto& src = src_list.landmarks[i];
    UnMotionMediaPipeLandmark& dst = dst_landmarks[i];
    dst.x = src.x;
    dst.y = src.y;
    dst.z = src.z;
    dst.visibility = src.visibility.value_or(score_for(src));
    dst.presence = src.presence.value_or(score_for(src));
    confidence_sum += score_for(src);
  }
  *out_confidence = count == 0 ? 0.0f : confidence_sum / static_cast<float>(count);
}

int32_t copy_pose_result(const pose::PoseLandmarkerResult& result, UnMotionMediaPipePose* out_pose) {
  if (out_pose == nullptr || result.pose_landmarks.empty()) {
    return 20;
  }

  const auto& first_pose = result.pose_landmarks[0];
  if (first_pose.landmarks.size() < 9) {
    return 21;
  }

  std::memset(out_pose, 0, sizeof(UnMotionMediaPipePose));
  copy_normalized_landmarks(first_pose, out_pose->landmarks, 33, &out_pose->landmark_count,
                            &out_pose->confidence);
  if (!result.pose_world_landmarks.empty()) {
    copy_world_landmarks(result.pose_world_landmarks[0], out_pose->world_landmarks, 33,
                         &out_pose->world_landmark_count);
  }

  if (result.segmentation_masks.has_value() && !result.segmentation_masks->empty()) {
    const auto& mask = (*result.segmentation_masks)[0];
    out_pose->segmentation_mask_present = 1;
    out_pose->segmentation_mask_width = static_cast<uint32_t>(std::max(mask.width(), 0));
    out_pose->segmentation_mask_height = static_cast<uint32_t>(std::max(mask.height(), 0));
  }
  return 0;
}

bool ascii_iequals(const std::string& a, const char* b) {
  if (a.size() != std::strlen(b)) {
    return false;
  }
  for (size_t i = 0; i < a.size(); ++i) {
    if (std::tolower(static_cast<unsigned char>(a[i])) !=
        std::tolower(static_cast<unsigned char>(b[i]))) {
      return false;
    }
  }
  return true;
}

bool env_string_truthy(const char* v) {
  if (v == nullptr || v[0] == '\0') {
    return false;
  }
  if (v[0] == '0' && v[1] == '\0') {
    return false;
  }
  const std::string s(v);
  return ascii_iequals(s, "1") || ascii_iequals(s, "true") || ascii_iequals(s, "yes") ||
         ascii_iequals(s, "on");
}

vision_core::RunningMode running_mode_from_options(uint32_t mode) {
  if (mode == UN_MOTION_MEDIAPIPE_RUNNING_MODE_IMAGE) {
    return vision_core::RunningMode::IMAGE;
  }
  if (mode == UN_MOTION_MEDIAPIPE_RUNNING_MODE_LIVE_STREAM) {
    return vision_core::RunningMode::LIVE_STREAM;
  }
  return vision_core::RunningMode::VIDEO;
}

bool option_enabled(uint8_t value) {
  return value != 0;
}

void configure_base_options(mediapipe::tasks::core::BaseOptions& base_options,
                            const UnMotionMediaPipeOptions& requested) {
  using BaseOptions = mediapipe::tasks::core::BaseOptions;
  if (requested.delegate == UN_MOTION_MEDIAPIPE_DELEGATE_GPU) {
    base_options.delegate = BaseOptions::Delegate::GPU;
    return;
  }
  base_options.delegate = BaseOptions::Delegate::CPU;
  if (requested.delegate == UN_MOTION_MEDIAPIPE_DELEGATE_XNNPACK) {
    BaseOptions::CpuOptions cpu_options;
    cpu_options.use_xnnpack = true;
    cpu_options.xnnpack_num_threads =
        requested.delegate_num_threads > 0
            ? static_cast<int>(std::min<uint32_t>(requested.delegate_num_threads,
                                                  static_cast<uint32_t>(std::numeric_limits<int>::max())))
            : -1;
    base_options.delegate_options = cpu_options;
  }
}

void put_env_kv(const char* key, const char* value) {
#if defined(_WIN32)
  (void)_putenv_s(key, value);
#else
  (void)setenv(key, value, 1);
#endif
}

void ApplyNativeLogSuppressionFromEnvOnce() {
  static bool applied = false;
  if (applied) {
    return;
  }
  applied = true;

  int from_level = 0;
  const char* level_env = std::getenv("UN_MOTION_MEDIAPIPE_LOG_LEVEL");
  if (level_env != nullptr && level_env[0] != '\0') {
    from_level = std::clamp(std::atoi(level_env), 0, 3);
  }
  int tf_level = from_level;
  if (env_string_truthy(std::getenv("UN_MOTION_MEDIAPIPE_QUIET"))) {
    tf_level = std::max(tf_level, 3);
  }
  if (tf_level > 0) {
    char buf[8];
    std::snprintf(buf, sizeof buf, "%d", tf_level);
    put_env_kv("TF_CPP_MIN_LOG_LEVEL", buf);
  }
  if (tf_level >= 2) {
    put_env_kv("GLOG_minloglevel", "2");
  }
}

bool NativeDiagStderrEnabled() {
  if (env_string_truthy(std::getenv("UN_MOTION_MEDIAPIPE_QUIET"))) {
    return false;
  }
  const char* level_env = std::getenv("UN_MOTION_MEDIAPIPE_LOG_LEVEL");
  if (level_env != nullptr && level_env[0] != '\0' &&
      std::clamp(std::atoi(level_env), 0, 3) >= 3) {
    return false;
  }
  return true;
}

uint8_t handedness_flag_from(const hand::HandLandmarkerResult& result, size_t hand_index) {
  if (hand_index >= result.handedness.size()) {
    return 255;
  }
  const auto& classifications = result.handedness[hand_index];
  if (classifications.categories.empty()) {
    return 255;
  }
  const auto& top = classifications.categories[0];
  const std::string* name = nullptr;
  if (top.category_name.has_value()) {
    name = &(*top.category_name);
  } else if (top.display_name.has_value()) {
    name = &(*top.display_name);
  }
  if (name == nullptr) {
    return 255;
  }
  if (ascii_iequals(*name, "right")) {
    return 1;
  }
  if (ascii_iequals(*name, "left")) {
    return 0;
  }
  return 255;
}

float handedness_score_from(const hand::HandLandmarkerResult& result, size_t hand_index) {
  if (hand_index >= result.handedness.size()) {
    return 0.0f;
  }
  const auto& classifications = result.handedness[hand_index];
  if (classifications.categories.empty()) {
    return 0.0f;
  }
  return classifications.categories[0].score;
}

int32_t copy_hands_result(const hand::HandLandmarkerResult& result, UnMotionMediaPipeHands* out_hands) {
  if (out_hands == nullptr) {
    return 0;
  }
  std::memset(out_hands, 0, sizeof(UnMotionMediaPipeHands));

  const size_t n = std::min(result.hand_landmarks.size(), static_cast<size_t>(UN_MOTION_MEDIAPIPE_MAX_HANDS));
  out_hands->hand_count = static_cast<uint32_t>(n);

  for (size_t h = 0; h < n; ++h) {
    UnMotionMediaPipeHand& dst_hand = out_hands->hands[h];
    const auto& src_list = result.hand_landmarks[h];
    const uint32_t lm_count =
        std::min<uint32_t>(static_cast<uint32_t>(src_list.landmarks.size()),
                           UN_MOTION_MEDIAPIPE_HAND_LANDMARK_COUNT);
    dst_hand.landmark_count = lm_count;
    dst_hand.handedness_score = handedness_score_from(result, h);
    dst_hand.handedness_is_right = handedness_flag_from(result, h);

    float confidence_sum = 0.0f;
    for (uint32_t i = 0; i < lm_count; ++i) {
      const auto& src = src_list.landmarks[i];
      UnMotionMediaPipeLandmark& dst = dst_hand.landmarks[i];
      dst.x = src.x;
      dst.y = src.y;
      dst.z = src.z;
      dst.visibility = src.visibility.value_or(score_for(src));
      dst.presence = src.presence.value_or(score_for(src));
      confidence_sum += score_for(src);
    }
    dst_hand.confidence = lm_count == 0 ? 0.0f : confidence_sum / static_cast<float>(lm_count);
    if (h < result.hand_world_landmarks.size()) {
      copy_world_landmarks(result.hand_world_landmarks[h], dst_hand.world_landmarks,
                           UN_MOTION_MEDIAPIPE_HAND_LANDMARK_COUNT,
                           &dst_hand.world_landmark_count);
    }
  }
  return 0;
}

void copy_name(char* dst, const std::string& src) {
  std::memset(dst, 0, UN_MOTION_MEDIAPIPE_BLENDSHAPE_NAME_BYTES);
  const size_t n = std::min(src.size(), static_cast<size_t>(UN_MOTION_MEDIAPIPE_BLENDSHAPE_NAME_BYTES - 1));
  std::memcpy(dst, src.data(), n);
}

int32_t copy_face_result(const face::FaceLandmarkerResult& result, UnMotionMediaPipeFace* out_face) {
  if (out_face == nullptr) {
    return 0;
  }
  std::memset(out_face, 0, sizeof(UnMotionMediaPipeFace));
  if (result.face_landmarks.empty()) {
    return 0;
  }

  const auto& first_face = result.face_landmarks[0];
  const uint32_t count =
      std::min<uint32_t>(static_cast<uint32_t>(first_face.landmarks.size()),
                         UN_MOTION_MEDIAPIPE_FACE_LANDMARK_COUNT);
  out_face->landmark_count = count;

  float confidence_sum = 0.0f;
  for (uint32_t i = 0; i < count; ++i) {
    const auto& src = first_face.landmarks[i];
    UnMotionMediaPipeLandmark& dst = out_face->landmarks[i];
    dst.x = src.x;
    dst.y = src.y;
    dst.z = src.z;
    dst.visibility = src.visibility.value_or(score_for(src));
    dst.presence = src.presence.value_or(score_for(src));
    confidence_sum += score_for(src);
  }
  out_face->confidence = count == 0 ? 0.0f : confidence_sum / static_cast<float>(count);

  if (result.facial_transformation_matrixes.has_value() &&
      !result.facial_transformation_matrixes->empty()) {
    const auto& matrix = (*result.facial_transformation_matrixes)[0];
    out_face->matrix_rows = static_cast<uint32_t>(matrix.rows());
    out_face->matrix_cols = static_cast<uint32_t>(matrix.cols());
    const uint32_t rows = std::min<uint32_t>(out_face->matrix_rows, 4);
    const uint32_t cols = std::min<uint32_t>(out_face->matrix_cols, 4);
    for (uint32_t r = 0; r < rows; ++r) {
      for (uint32_t c = 0; c < cols; ++c) {
        out_face->matrix[(r * 4) + c] = matrix(static_cast<int>(r), static_cast<int>(c));
      }
    }
  }

  if (result.face_blendshapes.has_value() && !result.face_blendshapes->empty()) {
    const auto& classifications = (*result.face_blendshapes)[0];
    const uint32_t n =
        std::min<uint32_t>(static_cast<uint32_t>(classifications.categories.size()),
                           UN_MOTION_MEDIAPIPE_MAX_FACE_BLENDSHAPES);
    out_face->blendshape_count = n;
    for (uint32_t i = 0; i < n; ++i) {
      const auto& category = classifications.categories[i];
      const std::string* name = nullptr;
      if (category.category_name.has_value()) {
        name = &(*category.category_name);
      } else if (category.display_name.has_value()) {
        name = &(*category.display_name);
      }
      if (name != nullptr) {
        copy_name(out_face->blendshapes[i].name, *name);
      }
      out_face->blendshapes[i].score = category.score;
    }
  }
  return 0;
}

void copy_category_name(char* dst, const mediapipe::Classification& src) {
  if (src.has_label() && !src.label().empty()) {
    copy_name(dst, src.label());
  } else if (src.has_display_name() && !src.display_name().empty()) {
    copy_name(dst, src.display_name());
  }
}

int32_t copy_gesture_result(const gesture::GestureRecognizerResult& result,
                            UnMotionMediaPipeGestures* out_gestures) {
  if (out_gestures == nullptr) {
    return 0;
  }
  std::memset(out_gestures, 0, sizeof(UnMotionMediaPipeGestures));
  const uint32_t n =
      std::min<uint32_t>(static_cast<uint32_t>(result.gestures.size()),
                         UN_MOTION_MEDIAPIPE_MAX_GESTURES);
  out_gestures->gesture_count = n;
  for (uint32_t i = 0; i < n; ++i) {
    UnMotionMediaPipeGesture& dst = out_gestures->gestures[i];
    const auto& categories = result.gestures[i].classification();
    const uint32_t c =
        std::min<uint32_t>(static_cast<uint32_t>(categories.size()),
                           UN_MOTION_MEDIAPIPE_MAX_GESTURE_CATEGORIES);
    dst.category_count = c;
    for (uint32_t j = 0; j < c; ++j) {
      copy_category_name(dst.categories[j].name, categories[j]);
      dst.categories[j].score = categories[j].score();
    }
    if (i < result.handedness.size() && result.handedness[i].classification_size() > 0) {
      const auto& handedness = result.handedness[i].classification(0);
      dst.handedness_score = handedness.score();
      if (handedness.has_label() && ascii_iequals(handedness.label(), "right")) {
        dst.handedness_is_right = 1;
      } else if (handedness.has_label() && ascii_iequals(handedness.label(), "left")) {
        dst.handedness_is_right = 0;
      } else {
        dst.handedness_is_right = 255;
      }
    } else {
      dst.handedness_is_right = 255;
    }
  }
  return 0;
}

void copy_holistic_hand(const mediapipe::tasks::components::containers::NormalizedLandmarks& landmarks,
                        const mediapipe::tasks::components::containers::Landmarks& world_landmarks,
                        uint8_t handedness_is_right,
                        UnMotionMediaPipeHand* out_hand) {
  std::memset(out_hand, 0, sizeof(UnMotionMediaPipeHand));
  out_hand->handedness_is_right = handedness_is_right;
  out_hand->handedness_score = landmarks.landmarks.empty() ? 0.0f : 1.0f;
  copy_normalized_landmarks(landmarks, out_hand->landmarks, UN_MOTION_MEDIAPIPE_HAND_LANDMARK_COUNT,
                            &out_hand->landmark_count, &out_hand->confidence);
  copy_world_landmarks(world_landmarks, out_hand->world_landmarks,
                       UN_MOTION_MEDIAPIPE_HAND_LANDMARK_COUNT,
                       &out_hand->world_landmark_count);
}

int32_t copy_holistic_result(const holistic::HolisticLandmarkerResult& result,
                             UnMotionMediaPipeHolistic* out_holistic) {
  if (out_holistic == nullptr) {
    return 0;
  }
  std::memset(out_holistic, 0, sizeof(UnMotionMediaPipeHolistic));
  copy_normalized_landmarks(result.pose_landmarks, out_holistic->pose.landmarks, 33,
                            &out_holistic->pose.landmark_count,
                            &out_holistic->pose.confidence);
  copy_world_landmarks(result.pose_world_landmarks, out_holistic->pose.world_landmarks, 33,
                       &out_holistic->pose.world_landmark_count);
  if (result.pose_segmentation_masks.has_value()) {
    out_holistic->pose.segmentation_mask_present = 1;
    out_holistic->pose.segmentation_mask_width =
        static_cast<uint32_t>(std::max(result.pose_segmentation_masks->width(), 0));
    out_holistic->pose.segmentation_mask_height =
        static_cast<uint32_t>(std::max(result.pose_segmentation_masks->height(), 0));
  }

  copy_holistic_hand(result.left_hand_landmarks, result.left_hand_world_landmarks, 0,
                     &out_holistic->left_hand);
  copy_holistic_hand(result.right_hand_landmarks, result.right_hand_world_landmarks, 1,
                     &out_holistic->right_hand);

  copy_normalized_landmarks(result.face_landmarks, out_holistic->face.landmarks,
                            UN_MOTION_MEDIAPIPE_FACE_LANDMARK_COUNT,
                            &out_holistic->face.landmark_count,
                            &out_holistic->face.confidence);
  if (result.face_blendshapes.has_value()) {
    const uint32_t n =
        std::min<uint32_t>(static_cast<uint32_t>(result.face_blendshapes->size()),
                           UN_MOTION_MEDIAPIPE_MAX_FACE_BLENDSHAPES);
    out_holistic->face.blendshape_count = n;
    for (uint32_t i = 0; i < n; ++i) {
      const auto& category = (*result.face_blendshapes)[i];
      const std::string* name = nullptr;
      if (category.category_name.has_value()) {
        name = &(*category.category_name);
      } else if (category.display_name.has_value()) {
        name = &(*category.display_name);
      }
      if (name != nullptr) {
        copy_name(out_holistic->face.blendshapes[i].name, *name);
      }
      out_holistic->face.blendshapes[i].score = category.score;
    }
  }
  return 0;
}

mediapipe::Image make_image(const uint8_t* rgb, uint32_t width, uint32_t height, uint32_t stride) {
  auto frame = std::make_shared<mediapipe::ImageFrame>();
  frame->CopyPixelData(
      mediapipe::ImageFormat::SRGB,
      static_cast<int>(width),
      static_cast<int>(height),
      static_cast<int>(stride),
      rgb,
      mediapipe::ImageFrame::kDefaultAlignmentBoundary);
  return mediapipe::Image(frame);
}

UnMotionMediaPipeOptions default_options() {
  UnMotionMediaPipeOptions options = {};
  options.abi_size = sizeof(UnMotionMediaPipeOptions);
  options.running_mode = UN_MOTION_MEDIAPIPE_RUNNING_MODE_VIDEO;
  options.enable_pose = 1;
  options.enable_hands = 1;
  options.enable_face = 1;
  options.enable_gestures = 0;
  options.enable_holistic =
      env_string_truthy(std::getenv("UN_MOTION_MEDIAPIPE_ENABLE_HOLISTIC")) ? 1 : 0;
  options.output_pose_segmentation =
      env_string_truthy(std::getenv("UN_MOTION_MEDIAPIPE_POSE_SEGMENTATION")) ? 1 : 0;
  options.delegate = UN_MOTION_MEDIAPIPE_DELEGATE_CPU;
  options.delegate_num_threads = 0;
  options.holistic_flow_limiter_enabled = 1;
  options.holistic_flow_limiter_max_in_flight = 1;
  options.holistic_flow_limiter_max_in_queue = 1;

  const char* mode_env = std::getenv("UN_MOTION_MEDIAPIPE_RUNNING_MODE");
  if (mode_env != nullptr && mode_env[0] != '\0') {
    const std::string mode(mode_env);
    if (ascii_iequals(mode, "image")) {
      options.running_mode = UN_MOTION_MEDIAPIPE_RUNNING_MODE_IMAGE;
    } else if (ascii_iequals(mode, "live_stream") || ascii_iequals(mode, "livestream") ||
               ascii_iequals(mode, "live")) {
      options.running_mode = UN_MOTION_MEDIAPIPE_RUNNING_MODE_LIVE_STREAM;
    } else {
      options.running_mode = UN_MOTION_MEDIAPIPE_RUNNING_MODE_VIDEO;
    }
  }

  return options;
}

UnMotionMediaPipeOptions normalize_options(const UnMotionMediaPipeOptions* raw_options) {
  UnMotionMediaPipeOptions options = default_options();
  if (raw_options == nullptr || raw_options->abi_size < 2 * sizeof(uint32_t)) {
    return options;
  }

  options.running_mode = raw_options->running_mode;
  if (raw_options->abi_size >= sizeof(UnMotionMediaPipeOptions)) {
    options.enable_pose = raw_options->enable_pose;
    options.enable_hands = raw_options->enable_hands;
    options.enable_face = raw_options->enable_face;
    options.enable_gestures = raw_options->enable_gestures;
    options.enable_holistic = raw_options->enable_holistic;
    options.output_pose_segmentation = raw_options->output_pose_segmentation;
  }
  if (raw_options->abi_size >= offsetof(UnMotionMediaPipeOptions, delegate) + sizeof(raw_options->delegate)) {
    options.delegate = raw_options->delegate;
  }
  if (raw_options->abi_size >= offsetof(UnMotionMediaPipeOptions, delegate_num_threads) + sizeof(raw_options->delegate_num_threads)) {
    options.delegate_num_threads = raw_options->delegate_num_threads;
  }
  if (raw_options->abi_size >= offsetof(UnMotionMediaPipeOptions, holistic_flow_limiter_enabled) + sizeof(raw_options->holistic_flow_limiter_enabled)) {
    options.holistic_flow_limiter_enabled = raw_options->holistic_flow_limiter_enabled;
  }
  if (raw_options->abi_size >= offsetof(UnMotionMediaPipeOptions, holistic_flow_limiter_max_in_flight) + sizeof(raw_options->holistic_flow_limiter_max_in_flight)) {
    options.holistic_flow_limiter_max_in_flight = raw_options->holistic_flow_limiter_max_in_flight;
  }
  if (raw_options->abi_size >= offsetof(UnMotionMediaPipeOptions, holistic_flow_limiter_max_in_queue) + sizeof(raw_options->holistic_flow_limiter_max_in_queue)) {
    options.holistic_flow_limiter_max_in_queue = raw_options->holistic_flow_limiter_max_in_queue;
  }
  if (options.delegate > UN_MOTION_MEDIAPIPE_DELEGATE_GPU) {
    options.delegate = UN_MOTION_MEDIAPIPE_DELEGATE_CPU;
  }
  options.holistic_flow_limiter_max_in_flight =
      std::max<uint32_t>(1, options.holistic_flow_limiter_max_in_flight);
  return options;
}

}  // namespace

extern "C" {

void* un_motion_mediapipe_create(void) {
  const UnMotionMediaPipeOptions options = default_options();
  return un_motion_mediapipe_create_with_options(&options);
}

void* un_motion_mediapipe_create_with_options(const UnMotionMediaPipeOptions* raw_options) {
  ApplyNativeLogSuppressionFromEnvOnce();
  const UnMotionMediaPipeOptions requested = normalize_options(raw_options);
  auto context = std::make_unique<UnMotionMediaPipeContext>();
  context->running_mode = running_mode_from_options(requested.running_mode);

  const bool holistic_enabled = option_enabled(requested.enable_holistic);

  if (!holistic_enabled && option_enabled(requested.enable_pose)) {
    auto options = std::make_unique<pose::PoseLandmarkerOptions>();
    options->base_options.model_asset_path = pose_model_path();
    configure_base_options(options->base_options, requested);
    options->base_options.host_environment = mediapipe::tasks::core::HOST_ENVIRONMENT_UNKNOWN;
    options->base_options.host_system = mediapipe::tasks::core::HOST_SYSTEM_WINDOWS;
    options->running_mode = context->running_mode;
    options->num_poses = 1;
    options->min_pose_detection_confidence = 0.5f;
    options->min_pose_presence_confidence = 0.5f;
    options->min_tracking_confidence = 0.5f;
    options->output_segmentation_masks = option_enabled(requested.output_pose_segmentation);
    if (context->running_mode == vision_core::RunningMode::LIVE_STREAM) {
      UnMotionMediaPipeContext* callback_context = context.get();
      options->result_callback =
          [callback_context](absl::StatusOr<pose::PoseLandmarkerResult> result,
                             const mediapipe::Image&, int64_t timestamp_ms) {
            UnMotionMediaPipePose copied = {};
            int32_t error = 13;
            if (result.ok()) {
              error = copy_pose_result(*result, &copied);
            }
            std::lock_guard<std::mutex> lock(callback_context->latest_mutex);
            callback_context->latest_pose = copied;
            callback_context->latest_pose_error = error;
            callback_context->latest_pose_timestamp_ms = timestamp_ms;
            callback_context->pose_callback_count += 1;
            push_bounded(callback_context->queued_pose, QueuedPoseResult{copied, error, timestamp_ms});
            callback_context->latest_pose_ready = true;
          };
    }

    auto landmarker = pose::PoseLandmarker::Create(std::move(options));
    if (!landmarker.ok()) {
      if (NativeDiagStderrEnabled()) {
        std::fprintf(stderr,
                     "un_motion_mediapipe_create: pose landmarker create failed: %s\n",
                     landmarker.status().ToString().c_str());
        std::fflush(stderr);
      }
      return nullptr;
    }
    context->pose_landmarker = std::move(*landmarker);
  }

  if (!holistic_enabled && option_enabled(requested.enable_face)) {
    auto options = std::make_unique<face::FaceLandmarkerOptions>();
    options->base_options.model_asset_path = face_model_path();
    configure_base_options(options->base_options, requested);
    options->base_options.host_environment = mediapipe::tasks::core::HOST_ENVIRONMENT_UNKNOWN;
    options->base_options.host_system = mediapipe::tasks::core::HOST_SYSTEM_WINDOWS;
    options->running_mode = context->running_mode;
    options->num_faces = 1;
    options->min_face_detection_confidence = 0.5f;
    options->min_face_presence_confidence = 0.5f;
    options->min_tracking_confidence = 0.5f;
    options->output_face_blendshapes = true;
    options->output_facial_transformation_matrixes = true;
    if (context->running_mode == vision_core::RunningMode::LIVE_STREAM) {
      UnMotionMediaPipeContext* callback_context = context.get();
      options->result_callback =
          [callback_context](absl::StatusOr<face::FaceLandmarkerResult> result,
                             const mediapipe::Image&, int64_t timestamp_ms) {
            UnMotionMediaPipeFace copied = {};
            int32_t error = 42;
            if (result.ok()) {
              error = copy_face_result(*result, &copied);
            }
            std::lock_guard<std::mutex> lock(callback_context->latest_mutex);
            callback_context->latest_face = copied;
            callback_context->latest_face_error = error;
            callback_context->latest_face_timestamp_ms = timestamp_ms;
            callback_context->face_callback_count += 1;
            push_bounded(callback_context->queued_face, QueuedFaceResult{copied, error, timestamp_ms});
            callback_context->latest_face_ready = true;
          };
    }

    auto landmarker = face::FaceLandmarker::Create(std::move(options));
    if (!landmarker.ok()) {
      if (NativeDiagStderrEnabled()) {
        std::fprintf(stderr,
                     "un_motion_mediapipe_create: face landmarker create failed: %s\n",
                     landmarker.status().ToString().c_str());
        std::fflush(stderr);
      }
      return nullptr;
    }
    context->face_landmarker = std::move(*landmarker);
  }

  if (option_enabled(requested.enable_gestures)) {
    auto options = std::make_unique<gesture::GestureRecognizerOptions>();
    options->base_options.model_asset_path = gesture_model_path();
    configure_base_options(options->base_options, requested);
    options->base_options.host_environment = mediapipe::tasks::core::HOST_ENVIRONMENT_UNKNOWN;
    options->base_options.host_system = mediapipe::tasks::core::HOST_SYSTEM_WINDOWS;
    options->running_mode = context->running_mode;
    options->num_hands = UN_MOTION_MEDIAPIPE_MAX_GESTURES;
    options->min_hand_detection_confidence = 0.5f;
    options->min_hand_presence_confidence = 0.5f;
    options->min_tracking_confidence = 0.5f;
    if (context->running_mode == vision_core::RunningMode::LIVE_STREAM) {
      UnMotionMediaPipeContext* callback_context = context.get();
      options->result_callback =
          [callback_context](absl::StatusOr<gesture::GestureRecognizerResult> result,
                             const mediapipe::Image&, int64_t timestamp_ms) {
            UnMotionMediaPipeGestures copied = {};
            int32_t error = 62;
            if (result.ok()) {
              error = copy_gesture_result(*result, &copied);
            }
            std::lock_guard<std::mutex> lock(callback_context->latest_mutex);
            callback_context->latest_gestures = copied;
            callback_context->latest_gestures_error = error;
            callback_context->latest_gestures_timestamp_ms = timestamp_ms;
            callback_context->gestures_callback_count += 1;
            push_bounded(callback_context->queued_gestures, QueuedGesturesResult{copied, error, timestamp_ms});
            callback_context->latest_gestures_ready = true;
          };
    }

    auto recognizer = gesture::GestureRecognizer::Create(std::move(options));
    if (!recognizer.ok()) {
      if (NativeDiagStderrEnabled()) {
        std::fprintf(stderr,
                     "un_motion_mediapipe_create: gesture recognizer create failed: %s\n",
                     recognizer.status().ToString().c_str());
        std::fflush(stderr);
      }
    } else {
      context->gesture_recognizer = std::move(*recognizer);
    }
  }

  if (holistic_enabled) {
    auto options = std::make_unique<holistic::HolisticLandmarkerOptions>();
    options->base_options.model_asset_path = holistic_model_path();
    configure_base_options(options->base_options, requested);
    options->base_options.host_environment = mediapipe::tasks::core::HOST_ENVIRONMENT_UNKNOWN;
    options->base_options.host_system = mediapipe::tasks::core::HOST_SYSTEM_WINDOWS;
    options->running_mode = context->running_mode;
    options->min_face_detection_confidence = 0.5f;
    options->min_face_presence_confidence = 0.5f;
    options->min_hand_landmarks_confidence = 0.5f;
    options->min_pose_detection_confidence = 0.5f;
    options->min_pose_presence_confidence = 0.5f;
    options->output_face_landmarks = option_enabled(requested.enable_face);
    options->output_face_blendshapes = option_enabled(requested.enable_face);
    options->output_hand_landmarks = option_enabled(requested.enable_hands);
    options->output_hand_world_landmarks = option_enabled(requested.enable_hands);
    options->output_pose_landmarks = true;
    options->output_pose_world_landmarks = true;
    options->output_pose_segmentation_masks = option_enabled(requested.output_pose_segmentation);
    options->flow_limiter_enabled = option_enabled(requested.holistic_flow_limiter_enabled);
    options->flow_limiter_max_in_flight =
        static_cast<int>(std::min<uint32_t>(requested.holistic_flow_limiter_max_in_flight,
                                            static_cast<uint32_t>(std::numeric_limits<int>::max())));
    options->flow_limiter_max_in_queue =
        static_cast<int>(std::min<uint32_t>(requested.holistic_flow_limiter_max_in_queue,
                                            static_cast<uint32_t>(std::numeric_limits<int>::max())));
    if (context->running_mode == vision_core::RunningMode::LIVE_STREAM) {
      UnMotionMediaPipeContext* callback_context = context.get();
      options->result_callback =
          [callback_context](absl::StatusOr<holistic::HolisticLandmarkerResult> result,
                             const mediapipe::Image&, int64_t timestamp_ms) {
            UnMotionMediaPipeHolistic copied = {};
            int32_t error = 72;
            if (result.ok()) {
              error = copy_holistic_result(*result, &copied);
            }
            std::lock_guard<std::mutex> lock(callback_context->latest_mutex);
            callback_context->latest_holistic = copied;
            callback_context->latest_holistic_error = error;
            callback_context->latest_holistic_timestamp_ms = timestamp_ms;
            callback_context->holistic_callback_count += 1;
            push_bounded(callback_context->queued_holistic, QueuedHolisticResult{copied, error, timestamp_ms});
            callback_context->latest_holistic_ready = true;
          };
    }

    auto landmarker = holistic::HolisticLandmarker::Create(std::move(options));
    if (!landmarker.ok()) {
      if (NativeDiagStderrEnabled()) {
        std::fprintf(stderr,
                     "un_motion_mediapipe_create: holistic landmarker create failed: %s\n",
                     landmarker.status().ToString().c_str());
        std::fflush(stderr);
      }
    } else {
      context->holistic_landmarker = std::move(*landmarker);
    }
  }

  if (!holistic_enabled && option_enabled(requested.enable_hands)) {
    auto options = std::make_unique<hand::HandLandmarkerOptions>();
    options->base_options.model_asset_path = hand_model_path();
    configure_base_options(options->base_options, requested);
    options->base_options.host_environment = mediapipe::tasks::core::HOST_ENVIRONMENT_UNKNOWN;
    options->base_options.host_system = mediapipe::tasks::core::HOST_SYSTEM_WINDOWS;
    options->running_mode = context->running_mode;
    options->num_hands = UN_MOTION_MEDIAPIPE_MAX_HANDS;
    options->min_hand_detection_confidence = 0.5f;
    options->min_hand_presence_confidence = 0.5f;
    options->min_tracking_confidence = 0.5f;
    if (context->running_mode == vision_core::RunningMode::LIVE_STREAM) {
      UnMotionMediaPipeContext* callback_context = context.get();
      options->result_callback =
          [callback_context](absl::StatusOr<hand::HandLandmarkerResult> result,
                             const mediapipe::Image&, int64_t timestamp_ms) {
            UnMotionMediaPipeHands copied = {};
            int32_t error = 52;
            if (result.ok()) {
              error = copy_hands_result(*result, &copied);
            }
            std::lock_guard<std::mutex> lock(callback_context->latest_mutex);
            callback_context->latest_hands = copied;
            callback_context->latest_hands_error = error;
            callback_context->latest_hands_timestamp_ms = timestamp_ms;
            callback_context->hands_callback_count += 1;
            push_bounded(callback_context->queued_hands, QueuedHandsResult{copied, error, timestamp_ms});
            callback_context->latest_hands_ready = true;
          };
    }

    auto landmarker = hand::HandLandmarker::Create(std::move(options));
    if (!landmarker.ok()) {
      if (NativeDiagStderrEnabled()) {
        std::fprintf(stderr,
                     "un_motion_mediapipe_create: hand landmarker create failed: %s\n",
                     landmarker.status().ToString().c_str());
        std::fflush(stderr);
      }
      return nullptr;
    }
    context->hand_landmarker = std::move(*landmarker);
  }

  return context.release();
}

void un_motion_mediapipe_destroy(void* raw_context) {
  if (raw_context == nullptr) {
    return;
  }

  auto* context = static_cast<UnMotionMediaPipeContext*>(raw_context);
  if (context->pose_landmarker) {
    (void)context->pose_landmarker->Close();
  }
  if (context->hand_landmarker) {
    (void)context->hand_landmarker->Close();
  }
  if (context->face_landmarker) {
    (void)context->face_landmarker->Close();
  }
  if (context->gesture_recognizer) {
    (void)context->gesture_recognizer->Close();
  }
  if (context->holistic_landmarker) {
    (void)context->holistic_landmarker->Close();
  }
  delete context;
}

int32_t un_motion_mediapipe_process_rgb_everything(
    void* raw_context,
    const uint8_t* rgb,
    uint32_t width,
    uint32_t height,
    uint32_t stride,
    UnMotionMediaPipePose* out_pose,
    UnMotionMediaPipeHands* out_hands,
    UnMotionMediaPipeFace* out_face,
    UnMotionMediaPipeGestures* out_gestures,
    UnMotionMediaPipeHolistic* out_holistic) {
  auto* context = static_cast<UnMotionMediaPipeContext*>(raw_context);
  const int64_t timestamp_ms = context == nullptr ? 0 : context->next_timestamp_ms + 1;
  return un_motion_mediapipe_process_rgb_everything_at(
      raw_context, rgb, width, height, stride, timestamp_ms, out_pose, out_hands, out_face,
      out_gestures, out_holistic);
}

int32_t un_motion_mediapipe_process_rgb_everything_at(
    void* raw_context,
    const uint8_t* rgb,
    uint32_t width,
    uint32_t height,
    uint32_t stride,
    int64_t timestamp_ms,
    UnMotionMediaPipePose* out_pose,
    UnMotionMediaPipeHands* out_hands,
    UnMotionMediaPipeFace* out_face,
    UnMotionMediaPipeGestures* out_gestures,
    UnMotionMediaPipeHolistic* out_holistic) {
  if (raw_context == nullptr || rgb == nullptr || width == 0 || height == 0) {
    return 10;
  }
  if (out_pose == nullptr && out_hands == nullptr && out_face == nullptr &&
      out_gestures == nullptr && out_holistic == nullptr) {
    return 10;
  }

  auto* context = static_cast<UnMotionMediaPipeContext*>(raw_context);
  if (timestamp_ms <= context->next_timestamp_ms) {
    timestamp_ms = context->next_timestamp_ms + 1;
  }
  context->next_timestamp_ms = timestamp_ms;
  if (out_pose != nullptr && !context->pose_landmarker) {
    return 11;
  }
  if (out_hands != nullptr && !context->hand_landmarker) {
    return 11;
  }
  if (out_face != nullptr && !context->face_landmarker) {
    return 11;
  }
  if (out_gestures != nullptr && !context->gesture_recognizer) {
    return 11;
  }
  if (out_holistic != nullptr && !context->holistic_landmarker) {
    return 11;
  }

  mediapipe::Image image = make_image(rgb, width, height, stride);
  int32_t first_error = 0;

  if (out_pose != nullptr) {
    if (context->running_mode == vision_core::RunningMode::IMAGE) {
      auto result = context->pose_landmarker->Detect(image);
      if (!result.ok()) {
        first_error = 13;
        std::memset(out_pose, 0, sizeof(*out_pose));
      } else {
        const int32_t pose_rc = copy_pose_result(*result, out_pose);
        if (pose_rc != 0) {
          std::memset(out_pose, 0, sizeof(*out_pose));
          first_error = pose_rc;
        }
      }
    } else if (context->running_mode == vision_core::RunningMode::LIVE_STREAM) {
      absl::Status status = context->pose_landmarker->DetectAsync(image, timestamp_ms);
      {
        std::lock_guard<std::mutex> lock(context->latest_mutex);
        context->pose_submit_count += 1;
        if (!status.ok()) {
          context->pose_submit_error_count += 1;
        }
      }
      if (!status.ok()) {
        first_error = 13;
      }
      std::memset(out_pose, 0, sizeof(*out_pose));
    } else {
      auto result = context->pose_landmarker->DetectForVideo(image, timestamp_ms);
      if (!result.ok()) {
        first_error = 13;
        std::memset(out_pose, 0, sizeof(*out_pose));
      } else {
        const int32_t pose_rc = copy_pose_result(*result, out_pose);
        if (pose_rc != 0) {
          std::memset(out_pose, 0, sizeof(*out_pose));
          first_error = pose_rc;
        }
      }
    }
    // Pose-only callers (un_motion_mediapipe_process_rgb) keep pose error codes.
    if (first_error != 0 && out_hands == nullptr) {
      return first_error;
    }
  }

  if (out_hands != nullptr) {
    if (context->running_mode == vision_core::RunningMode::IMAGE) {
      auto result = context->hand_landmarker->Detect(image);
      if (!result.ok()) {
        first_error = 52;
        std::memset(out_hands, 0, sizeof(*out_hands));
      } else {
        (void)copy_hands_result(*result, out_hands);
      }
    } else if (context->running_mode == vision_core::RunningMode::LIVE_STREAM) {
      absl::Status status = context->hand_landmarker->DetectAsync(image, timestamp_ms);
      {
        std::lock_guard<std::mutex> lock(context->latest_mutex);
        context->hands_submit_count += 1;
        if (!status.ok()) {
          context->hands_submit_error_count += 1;
        }
      }
      if (!status.ok()) {
        first_error = 52;
      }
      std::memset(out_hands, 0, sizeof(*out_hands));
    } else {
      auto result = context->hand_landmarker->DetectForVideo(image, timestamp_ms);
      if (!result.ok()) {
        first_error = 52;
      } else {
        (void)copy_hands_result(*result, out_hands);
      }
    }
  }

  if (out_face != nullptr) {
    if (context->running_mode == vision_core::RunningMode::IMAGE) {
      auto result = context->face_landmarker->Detect(image);
      if (!result.ok()) {
        first_error = 42;
        std::memset(out_face, 0, sizeof(*out_face));
      } else {
        (void)copy_face_result(*result, out_face);
      }
    } else if (context->running_mode == vision_core::RunningMode::LIVE_STREAM) {
      absl::Status status = context->face_landmarker->DetectAsync(image, timestamp_ms);
      {
        std::lock_guard<std::mutex> lock(context->latest_mutex);
        context->face_submit_count += 1;
        if (!status.ok()) {
          context->face_submit_error_count += 1;
        }
      }
      if (!status.ok()) {
        first_error = 42;
      }
      std::memset(out_face, 0, sizeof(*out_face));
    } else {
      auto result = context->face_landmarker->DetectForVideo(image, timestamp_ms);
      if (!result.ok()) {
        first_error = 42;
        std::memset(out_face, 0, sizeof(*out_face));
      } else {
        (void)copy_face_result(*result, out_face);
      }
    }
  }

  if (out_gestures != nullptr) {
    if (context->running_mode == vision_core::RunningMode::IMAGE) {
      auto result = context->gesture_recognizer->Recognize(image);
      if (!result.ok()) {
        first_error = 62;
        std::memset(out_gestures, 0, sizeof(*out_gestures));
      } else {
        (void)copy_gesture_result(*result, out_gestures);
      }
    } else if (context->running_mode == vision_core::RunningMode::LIVE_STREAM) {
      absl::Status status = context->gesture_recognizer->RecognizeAsync(image, timestamp_ms);
      {
        std::lock_guard<std::mutex> lock(context->latest_mutex);
        context->gestures_submit_count += 1;
        if (!status.ok()) {
          context->gestures_submit_error_count += 1;
        }
      }
      if (!status.ok()) {
        first_error = 62;
      }
      std::memset(out_gestures, 0, sizeof(*out_gestures));
    } else {
      auto result = context->gesture_recognizer->RecognizeForVideo(image, timestamp_ms);
      if (!result.ok()) {
        first_error = 62;
        std::memset(out_gestures, 0, sizeof(*out_gestures));
      } else {
        (void)copy_gesture_result(*result, out_gestures);
      }
    }
  }

  if (out_holistic != nullptr) {
    if (context->running_mode == vision_core::RunningMode::IMAGE) {
      auto result = context->holistic_landmarker->Detect(image);
      if (!result.ok()) {
        first_error = 72;
        std::memset(out_holistic, 0, sizeof(*out_holistic));
      } else {
        (void)copy_holistic_result(*result, out_holistic);
      }
    } else if (context->running_mode == vision_core::RunningMode::LIVE_STREAM) {
      absl::Status status = context->holistic_landmarker->DetectAsync(image, timestamp_ms);
      {
        std::lock_guard<std::mutex> lock(context->latest_mutex);
        context->holistic_submit_count += 1;
        if (!status.ok()) {
          context->holistic_submit_error_count += 1;
        }
      }
      if (!status.ok()) {
        first_error = 72;
      }
      std::memset(out_holistic, 0, sizeof(*out_holistic));
    } else {
      auto result = context->holistic_landmarker->DetectForVideo(image, timestamp_ms);
      if (!result.ok()) {
        first_error = 72;
        std::memset(out_holistic, 0, sizeof(*out_holistic));
      } else {
        (void)copy_holistic_result(*result, out_holistic);
      }
    }
  }

  return first_error;
}

int32_t un_motion_mediapipe_poll_latest_at(
    void* raw_context,
    int64_t timestamp_ms,
    UnMotionMediaPipePose* out_pose,
    UnMotionMediaPipeHands* out_hands,
    UnMotionMediaPipeFace* out_face,
    UnMotionMediaPipeGestures* out_gestures,
    UnMotionMediaPipeHolistic* out_holistic) {
  if (raw_context == nullptr) {
    return 10;
  }
  if (out_pose == nullptr && out_hands == nullptr && out_face == nullptr &&
      out_gestures == nullptr && out_holistic == nullptr) {
    return 10;
  }

  auto* context = static_cast<UnMotionMediaPipeContext*>(raw_context);
  std::lock_guard<std::mutex> lock(context->latest_mutex);
  int32_t first_error = 0;
  bool all_ready = true;

  if (out_pose != nullptr) {
    QueuedPoseResult item = {};
    if (pop_first_at_or_after(context->queued_pose, timestamp_ms, &item)) {
      *out_pose = item.value;
      if (item.error != 0 && first_error == 0) {
        first_error = item.error;
      }
    } else {
      std::memset(out_pose, 0, sizeof(*out_pose));
      all_ready = false;
    }
  }

  if (out_hands != nullptr) {
    QueuedHandsResult item = {};
    if (pop_first_at_or_after(context->queued_hands, timestamp_ms, &item)) {
      *out_hands = item.value;
      if (item.error != 0 && first_error == 0) {
        first_error = item.error;
      }
    } else {
      std::memset(out_hands, 0, sizeof(*out_hands));
      all_ready = false;
    }
  }

  if (out_face != nullptr) {
    QueuedFaceResult item = {};
    if (pop_first_at_or_after(context->queued_face, timestamp_ms, &item)) {
      *out_face = item.value;
      if (item.error != 0 && first_error == 0) {
        first_error = item.error;
      }
    } else {
      std::memset(out_face, 0, sizeof(*out_face));
      all_ready = false;
    }
  }

  if (out_gestures != nullptr) {
    QueuedGesturesResult item = {};
    if (pop_first_at_or_after(context->queued_gestures, timestamp_ms, &item)) {
      *out_gestures = item.value;
      if (item.error != 0 && first_error == 0) {
        first_error = item.error;
      }
    } else {
      std::memset(out_gestures, 0, sizeof(*out_gestures));
      all_ready = false;
    }
  }

  if (out_holistic != nullptr) {
    QueuedHolisticResult item = {};
    if (pop_first_at_or_after(context->queued_holistic, timestamp_ms, &item)) {
      *out_holistic = item.value;
      if (item.error != 0 && first_error == 0) {
        first_error = item.error;
      }
    } else {
      std::memset(out_holistic, 0, sizeof(*out_holistic));
      all_ready = false;
    }
  }

  if (!all_ready && first_error == 0) {
    return 30;
  }
  return first_error;
}

int32_t un_motion_mediapipe_poll_latest_timestamp_at(
    void* raw_context,
    int64_t timestamp_ms,
    UnMotionMediaPipePose* out_pose,
    UnMotionMediaPipeHands* out_hands,
    UnMotionMediaPipeFace* out_face,
    UnMotionMediaPipeGestures* out_gestures,
    UnMotionMediaPipeHolistic* out_holistic,
    int64_t* out_result_timestamp_ms) {
  if (out_result_timestamp_ms == nullptr) {
    return 10;
  }
  *out_result_timestamp_ms = -1;
  if (raw_context == nullptr) {
    return 10;
  }
  if (out_pose == nullptr && out_hands == nullptr && out_face == nullptr &&
      out_gestures == nullptr && out_holistic == nullptr) {
    return 10;
  }

  auto* context = static_cast<UnMotionMediaPipeContext*>(raw_context);
  std::lock_guard<std::mutex> lock(context->latest_mutex);
  int32_t first_error = 0;
  bool all_ready = true;
  int64_t result_timestamp_ms = std::numeric_limits<int64_t>::max();

  if (out_pose != nullptr) {
    QueuedPoseResult item = {};
    if (pop_first_at_or_after(context->queued_pose, timestamp_ms, &item)) {
      *out_pose = item.value;
      result_timestamp_ms = std::min(result_timestamp_ms, item.timestamp_ms);
      if (item.error != 0 && first_error == 0) {
        first_error = item.error;
      }
    } else {
      std::memset(out_pose, 0, sizeof(*out_pose));
      all_ready = false;
    }
  }

  if (out_hands != nullptr) {
    QueuedHandsResult item = {};
    if (pop_first_at_or_after(context->queued_hands, timestamp_ms, &item)) {
      *out_hands = item.value;
      result_timestamp_ms = std::min(result_timestamp_ms, item.timestamp_ms);
      if (item.error != 0 && first_error == 0) {
        first_error = item.error;
      }
    } else {
      std::memset(out_hands, 0, sizeof(*out_hands));
      all_ready = false;
    }
  }

  if (out_face != nullptr) {
    QueuedFaceResult item = {};
    if (pop_first_at_or_after(context->queued_face, timestamp_ms, &item)) {
      *out_face = item.value;
      result_timestamp_ms = std::min(result_timestamp_ms, item.timestamp_ms);
      if (item.error != 0 && first_error == 0) {
        first_error = item.error;
      }
    } else {
      std::memset(out_face, 0, sizeof(*out_face));
      all_ready = false;
    }
  }

  if (out_gestures != nullptr) {
    QueuedGesturesResult item = {};
    if (pop_first_at_or_after(context->queued_gestures, timestamp_ms, &item)) {
      *out_gestures = item.value;
      result_timestamp_ms = std::min(result_timestamp_ms, item.timestamp_ms);
      if (item.error != 0 && first_error == 0) {
        first_error = item.error;
      }
    } else {
      std::memset(out_gestures, 0, sizeof(*out_gestures));
      all_ready = false;
    }
  }

  if (out_holistic != nullptr) {
    QueuedHolisticResult item = {};
    if (pop_first_at_or_after(context->queued_holistic, timestamp_ms, &item)) {
      *out_holistic = item.value;
      result_timestamp_ms = std::min(result_timestamp_ms, item.timestamp_ms);
      if (item.error != 0 && first_error == 0) {
        first_error = item.error;
      }
    } else {
      std::memset(out_holistic, 0, sizeof(*out_holistic));
      all_ready = false;
    }
  }

  if (!all_ready && first_error == 0) {
    return 30;
  }
  if (result_timestamp_ms != std::numeric_limits<int64_t>::max()) {
    *out_result_timestamp_ms = result_timestamp_ms;
  }
  return first_error;
}

int32_t un_motion_mediapipe_live_stream_stats(
    void* raw_context,
    UnMotionMediaPipeLiveStreamStats* out_stats) {
  if (raw_context == nullptr || out_stats == nullptr) {
    return 10;
  }

  auto* context = static_cast<UnMotionMediaPipeContext*>(raw_context);
  std::lock_guard<std::mutex> lock(context->latest_mutex);
  out_stats->pose_submit_count = context->pose_submit_count;
  out_stats->hands_submit_count = context->hands_submit_count;
  out_stats->face_submit_count = context->face_submit_count;
  out_stats->gestures_submit_count = context->gestures_submit_count;
  out_stats->holistic_submit_count = context->holistic_submit_count;
  out_stats->pose_submit_error_count = context->pose_submit_error_count;
  out_stats->hands_submit_error_count = context->hands_submit_error_count;
  out_stats->face_submit_error_count = context->face_submit_error_count;
  out_stats->gestures_submit_error_count = context->gestures_submit_error_count;
  out_stats->holistic_submit_error_count = context->holistic_submit_error_count;
  out_stats->pose_callback_count = context->pose_callback_count;
  out_stats->hands_callback_count = context->hands_callback_count;
  out_stats->face_callback_count = context->face_callback_count;
  out_stats->gestures_callback_count = context->gestures_callback_count;
  out_stats->holistic_callback_count = context->holistic_callback_count;
  out_stats->latest_pose_timestamp_ms = context->latest_pose_timestamp_ms;
  out_stats->latest_hands_timestamp_ms = context->latest_hands_timestamp_ms;
  out_stats->latest_face_timestamp_ms = context->latest_face_timestamp_ms;
  out_stats->latest_gestures_timestamp_ms = context->latest_gestures_timestamp_ms;
  out_stats->latest_holistic_timestamp_ms = context->latest_holistic_timestamp_ms;
  return 0;
}

int32_t un_motion_mediapipe_process_rgb_full(
    void* raw_context,
    const uint8_t* rgb,
    uint32_t width,
    uint32_t height,
    uint32_t stride,
    UnMotionMediaPipePose* out_pose,
    UnMotionMediaPipeHands* out_hands,
    UnMotionMediaPipeFace* out_face) {
  return un_motion_mediapipe_process_rgb_everything(
      raw_context, rgb, width, height, stride, out_pose, out_hands, out_face, nullptr, nullptr);
}

int32_t un_motion_mediapipe_process_rgb_pose_and_hands(
    void* raw_context,
    const uint8_t* rgb,
    uint32_t width,
    uint32_t height,
    uint32_t stride,
    UnMotionMediaPipePose* out_pose,
    UnMotionMediaPipeHands* out_hands) {
  return un_motion_mediapipe_process_rgb_full(
      raw_context, rgb, width, height, stride, out_pose, out_hands, nullptr);
}

int32_t un_motion_mediapipe_process_rgb(
    void* raw_context,
    const uint8_t* rgb,
    uint32_t width,
    uint32_t height,
    uint32_t stride,
    UnMotionMediaPipePose* out_pose) {
  return un_motion_mediapipe_process_rgb_pose_and_hands(
      raw_context, rgb, width, height, stride, out_pose, nullptr);
}

}  // extern "C"
