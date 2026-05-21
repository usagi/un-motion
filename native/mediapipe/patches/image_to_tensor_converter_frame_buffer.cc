// Copyright 2023 The MediaPipe Authors.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#include "mediapipe/calculators/tensor/image_to_tensor_converter_frame_buffer.h"

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <memory>

#include "absl/status/status.h"
#include "absl/status/statusor.h"
#include "absl/strings/str_format.h"
#include "mediapipe/calculators/tensor/image_to_tensor_converter.h"
#include "mediapipe/calculators/tensor/image_to_tensor_utils.h"
#include "mediapipe/framework/calculator_context.h"
#include "mediapipe/framework/formats/frame_buffer.h"
#include "mediapipe/framework/formats/image.h"
#include "mediapipe/framework/formats/tensor.h"
#include "mediapipe/framework/port/status_macros.h"
#include "mediapipe/gpu/frame_buffer_view.h"

namespace mediapipe {

namespace {

class ImageToTensorFrameBufferConverter : public ImageToTensorConverter {
 public:
  ImageToTensorFrameBufferConverter(BorderMode border_mode,
                                    Tensor::ElementType tensor_type)
      : border_mode_(border_mode), tensor_type_(tensor_type) {}

  absl::Status Convert(const mediapipe::Image& input, const RotatedRect& roi,
                       float range_min, float range_max,
                       int tensor_buffer_offset,
                       Tensor& output_tensor) override;

 private:
  absl::Status ValidateTensorShape(const Tensor::Shape& shape);
  absl::Status ValidateInputFrame(const FrameBuffer& frame);
  float SampleChannel(const FrameBuffer& input, float x, float y,
                      int channel) const;
  float PixelOrBorder(const FrameBuffer& input, int x, int y,
                      int channel) const;
  void WriteUint8(const FrameBuffer& input, const RotatedRect& roi,
                  uint8_t* output, int output_width, int output_height,
                  int output_channels) const;
  absl::Status WriteFloat(const FrameBuffer& input, const RotatedRect& roi,
                          float range_min, float range_max, float* output,
                          int output_width, int output_height,
                          int output_channels) const;

  BorderMode border_mode_;
  Tensor::ElementType tensor_type_;
};

absl::Status ImageToTensorFrameBufferConverter::Convert(
    const mediapipe::Image& input, const RotatedRect& roi, float range_min,
    float range_max, int tensor_buffer_offset, Tensor& output_tensor) {
  RET_CHECK_EQ(tensor_buffer_offset, 0)
      << "Non-zero tensor_buffer_offset input is not supported.";

  if (tensor_type_ == Tensor::ElementType::kUInt8) {
    RET_CHECK(static_cast<int>(range_min) == 0 &&
              static_cast<int>(range_max) == 255);
  }

  auto input_frame =
      input.GetGpuBuffer(/*upload_to_gpu=*/false).GetReadView<FrameBuffer>();
  MP_RETURN_IF_ERROR(ValidateInputFrame(*input_frame));

  const auto& output_shape = output_tensor.shape();
  MP_RETURN_IF_ERROR(ValidateTensorShape(output_shape));
  const int output_height = output_shape.dims[1];
  const int output_width = output_shape.dims[2];
  const int output_channels = output_shape.dims[3];

  switch (tensor_type_) {
    case Tensor::ElementType::kUInt8: {
      auto view = output_tensor.GetCpuWriteView();
      WriteUint8(*input_frame, roi, view.buffer<uint8_t>(), output_width,
                 output_height, output_channels);
      return absl::OkStatus();
    }
    case Tensor::ElementType::kFloat32: {
      auto view = output_tensor.GetCpuWriteView();
      return WriteFloat(*input_frame, roi, range_min, range_max,
                        view.buffer<float>(), output_width, output_height,
                        output_channels);
    }
    default:
      return absl::InvalidArgumentError(
          absl::StrFormat("Tensor type is not supported by "
                          "ImageToTensorFrameBufferConverter, type: %d.",
                          static_cast<int>(tensor_type_)));
  }
}

absl::Status ImageToTensorFrameBufferConverter::ValidateTensorShape(
    const Tensor::Shape& shape) {
  RET_CHECK_EQ(shape.dims.size(), 4)
      << "Wrong output dims size: " << shape.dims.size();
  RET_CHECK_EQ(shape.dims[0], 1)
      << "Handling batch dimension not equal to 1 is not implemented.";
  RET_CHECK(shape.dims[3] == 1 || shape.dims[3] == 3)
      << "Wrong output channel: " << shape.dims[3];
  return absl::OkStatus();
}

absl::Status ImageToTensorFrameBufferConverter::ValidateInputFrame(
    const FrameBuffer& frame) {
  if (frame.plane_count() != 1) {
    return absl::InvalidArgumentError(
        "Only single-plane frame buffers are supported.");
  }
  switch (frame.format()) {
    case FrameBuffer::Format::kRGB:
    case FrameBuffer::Format::kRGBA:
    case FrameBuffer::Format::kGRAY:
      return absl::OkStatus();
    default:
      return absl::InvalidArgumentError(
          absl::StrFormat("Unsupported frame buffer format: %d.",
                          static_cast<int>(frame.format())));
  }
}

float ImageToTensorFrameBufferConverter::PixelOrBorder(
    const FrameBuffer& input, int x, int y, int channel) const {
  const auto dimension = input.dimension();
  if (x < 0 || y < 0 || x >= dimension.width || y >= dimension.height) {
    if (border_mode_ == BorderMode::kZero) {
      return 0.0f;
    }
    x = std::clamp(x, 0, dimension.width - 1);
    y = std::clamp(y, 0, dimension.height - 1);
  }

  const auto& plane = input.plane(0);
  const uint8_t* row =
      plane.buffer() + (y * plane.stride().row_stride_bytes);
  switch (input.format()) {
    case FrameBuffer::Format::kRGB:
      return static_cast<float>(
          row[x * plane.stride().pixel_stride_bytes + std::min(channel, 2)]);
    case FrameBuffer::Format::kRGBA:
      return static_cast<float>(
          row[x * plane.stride().pixel_stride_bytes + std::min(channel, 2)]);
    case FrameBuffer::Format::kGRAY:
      return static_cast<float>(row[x * plane.stride().pixel_stride_bytes]);
    default:
      return 0.0f;
  }
}

float ImageToTensorFrameBufferConverter::SampleChannel(
    const FrameBuffer& input, float x, float y, int channel) const {
  const int x0 = static_cast<int>(std::floor(x));
  const int y0 = static_cast<int>(std::floor(y));
  const int x1 = x0 + 1;
  const int y1 = y0 + 1;
  const float wx = x - static_cast<float>(x0);
  const float wy = y - static_cast<float>(y0);

  const float p00 = PixelOrBorder(input, x0, y0, channel);
  const float p10 = PixelOrBorder(input, x1, y0, channel);
  const float p01 = PixelOrBorder(input, x0, y1, channel);
  const float p11 = PixelOrBorder(input, x1, y1, channel);
  const float top = p00 + (p10 - p00) * wx;
  const float bottom = p01 + (p11 - p01) * wx;
  return top + (bottom - top) * wy;
}

void ImageToTensorFrameBufferConverter::WriteUint8(
    const FrameBuffer& input, const RotatedRect& roi, uint8_t* output,
    int output_width, int output_height, int output_channels) const {
  const float cos_theta = std::cos(roi.rotation);
  const float sin_theta = std::sin(roi.rotation);

  for (int y = 0; y < output_height; ++y) {
    const float local_y =
        ((static_cast<float>(y) + 0.5f) / static_cast<float>(output_height) -
         0.5f) *
        roi.height;
    for (int x = 0; x < output_width; ++x) {
      const float local_x =
          ((static_cast<float>(x) + 0.5f) / static_cast<float>(output_width) -
           0.5f) *
          roi.width;
      const float src_x =
          roi.center_x + (local_x * cos_theta) - (local_y * sin_theta);
      const float src_y =
          roi.center_y + (local_x * sin_theta) + (local_y * cos_theta);
      const int offset = ((y * output_width) + x) * output_channels;
      for (int channel = 0; channel < output_channels; ++channel) {
        const float value = SampleChannel(input, src_x, src_y, channel);
        output[offset + channel] = static_cast<uint8_t>(
            std::clamp(std::lround(value), 0l, 255l));
      }
    }
  }
}

absl::Status ImageToTensorFrameBufferConverter::WriteFloat(
    const FrameBuffer& input, const RotatedRect& roi, float range_min,
    float range_max, float* output, int output_width, int output_height,
    int output_channels) const {
  constexpr float kInputImageRangeMin = 0.0f;
  constexpr float kInputImageRangeMax = 255.0f;
  MP_ASSIGN_OR_RETURN(
      auto transform,
      GetValueRangeTransformation(kInputImageRangeMin, kInputImageRangeMax,
                                  range_min, range_max));
  const float cos_theta = std::cos(roi.rotation);
  const float sin_theta = std::sin(roi.rotation);

  for (int y = 0; y < output_height; ++y) {
    const float local_y =
        ((static_cast<float>(y) + 0.5f) / static_cast<float>(output_height) -
         0.5f) *
        roi.height;
    for (int x = 0; x < output_width; ++x) {
      const float local_x =
          ((static_cast<float>(x) + 0.5f) / static_cast<float>(output_width) -
           0.5f) *
          roi.width;
      const float src_x =
          roi.center_x + (local_x * cos_theta) - (local_y * sin_theta);
      const float src_y =
          roi.center_y + (local_x * sin_theta) + (local_y * cos_theta);
      const int offset = ((y * output_width) + x) * output_channels;
      for (int channel = 0; channel < output_channels; ++channel) {
        const float value = SampleChannel(input, src_x, src_y, channel);
        output[offset + channel] = value * transform.scale + transform.offset;
      }
    }
  }
  return absl::OkStatus();
}

}  // namespace

absl::StatusOr<std::unique_ptr<ImageToTensorConverter>>
CreateFrameBufferConverter(CalculatorContext* cc, BorderMode border_mode,
                           Tensor::ElementType tensor_type) {
  if (tensor_type != Tensor::ElementType::kUInt8 &&
      tensor_type != Tensor::ElementType::kFloat32) {
    return absl::InvalidArgumentError(
        absl::StrFormat("Tensor type is not supported by "
                        "ImageToTensorFrameBufferConverter, type: %d.",
                        static_cast<int>(tensor_type)));
  }
  return std::make_unique<ImageToTensorFrameBufferConverter>(border_mode,
                                                             tensor_type);
}

}  // namespace mediapipe
