use actix_cors::Cors;
use actix_web::{
	App, Error, HttpResponse, HttpServer, Responder, get,
	http::header,
	post, put,
	web::{self, Bytes},
};
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

use crate::{ActiveProfileRequest, CoreApiConfig, CoreControlState, CoreEvent, CoreProfileDocument, CoreSnapshot, RuntimeStatus};

const TELEMETRY_STREAM_INTERVAL_MS: u64 = 250;
const TAURI_WEBVIEW_ORIGINS: &[&str] = &["tauri://localhost", "http://tauri.localhost", "https://tauri.localhost"];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StatusResponse {
	pub status: RuntimeStatus,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotResponse {
	pub snapshot: CoreSnapshot,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDocumentResponse {
	pub selection: CoreProfileDocument,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDocumentRequest {
	pub selection: CoreProfileDocument,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NeutralCalibrationRequest {
	#[serde(default = "default_neutral_calibration_sample_count")]
	pub valid_sample_count: usize,
	#[serde(default)]
	pub pose: Option<String>,
}

fn default_neutral_calibration_sample_count() -> usize {
	45
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FacePoseModelBuildRequest {
	#[serde(default = "default_face_pose_model_sample_count")]
	pub valid_sample_count: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisExtrasCaptureRequest {
	pub output_dir: PathBuf,
	#[serde(default = "default_analysis_extras_duration_ms")]
	pub duration_ms: u64,
}

fn default_analysis_extras_duration_ms() -> u64 {
	1000
}

fn default_face_pose_model_sample_count() -> usize {
	90
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
	pub ok: bool,
}

pub fn configure_api(cfg: &mut web::ServiceConfig) {
	cfg.service(healthz)
		.service(exit_core)
		.service(get_status)
		.service(start_runtime)
		.service(stop_runtime)
		.service(calibrate_neutral)
		.service(clear_neutral_calibration)
		.service(build_face_pose_model)
		.service(capture_unmotion_pose)
		.service(capture_analysis_extras)
		.service(profiles)
		.service(profile_document)
		.service(set_profile_document)
		.service(sync_profile_document)
		.service(active_profile)
		.service(set_active_profile)
		.service(runtime_snapshot)
		.service(telemetry_stream)
		.service(events)
		.service(events_stream);
}

fn core_api_cors() -> Cors {
	Cors::default()
		.allowed_origin_fn(|origin, _req_head| is_allowed_core_api_origin(origin))
		.allowed_methods(["GET", "POST", "PUT", "OPTIONS"])
		.allowed_headers([header::CONTENT_TYPE])
		.max_age(3600)
}

fn is_allowed_core_api_origin(origin: &header::HeaderValue) -> bool {
	let Ok(origin) = origin.to_str() else {
		return false;
	};

	TAURI_WEBVIEW_ORIGINS.contains(&origin) || is_loopback_http_origin(origin)
}

fn is_loopback_http_origin(origin: &str) -> bool {
	let Some(authority) = origin
		.strip_prefix("http://")
		.or_else(|| origin.strip_prefix("https://"))
		.and_then(|origin| origin.split(['/', '?', '#']).next())
	else {
		return false;
	};

	let host = if authority.starts_with('[') {
		let Some(end) = authority.find(']') else {
			return false;
		};
		&authority[..=end]
	} else {
		authority.split(':').next().unwrap_or(authority)
	};

	matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

pub async fn run_api_server(config: CoreApiConfig, state: CoreControlState) -> std::io::Result<()> {
	config.validate().map_err(std::io::Error::other)?;
	let bind_addr = config.bind_addr;
	let auto_start_runtime = config.auto_start_runtime;
	let api_worker_threads = config.normalized_api_worker_threads();
	let state_for_auto_start = state.clone();
	let config_data = web::Data::new(config);
	let state_data = web::Data::new(state);
	if auto_start_runtime {
		// `selected_profile_id` の現在値で runtime を起動。失敗しても API server は
		// 上げて Supervisor からエラーを観測できるようにする (Capturer 終了は避ける)。
		let status = state_for_auto_start.start_runtime().await;
		if !status.running {
			tracing::warn!(
				target: "un_motion_core::server",
				health = %status.health,
				"auto_start_runtime requested but runtime did not start (continuing to serve API)",
			);
		} else {
			tracing::info!(
				target: "un_motion_core::server",
				active_profile_id = %status.active_profile_id,
				"auto_start_runtime succeeded",
			);
		}
	}
	HttpServer::new(move || {
		App::new()
			.wrap(core_api_cors())
			.app_data(config_data.clone())
			.app_data(state_data.clone())
			.configure(configure_api)
	})
	.workers(api_worker_threads)
	.bind(bind_addr)?
	.run()
	.await
}

#[get("/healthz")]
async fn healthz() -> impl Responder {
	web::Json(HealthResponse { ok: true })
}

#[post("/api/core/exit")]
async fn exit_core() -> impl Responder {
	actix_web::rt::spawn(async {
		tokio::time::sleep(Duration::from_millis(100)).await;
		std::process::exit(0);
	});
	web::Json(HealthResponse { ok: true })
}

#[get("/api/status")]
async fn get_status(state: web::Data<CoreControlState>) -> impl Responder {
	web::Json(StatusResponse {
		status: state.status().await,
	})
}

#[post("/api/runtime/start")]
async fn start_runtime(state: web::Data<CoreControlState>) -> impl Responder {
	web::Json(StatusResponse {
		status: state.start_runtime().await,
	})
}

#[post("/api/runtime/stop")]
async fn stop_runtime(state: web::Data<CoreControlState>) -> impl Responder {
	web::Json(StatusResponse {
		status: state.stop_runtime().await,
	})
}

#[post("/api/runtime/calibration/neutral")]
async fn calibrate_neutral(state: web::Data<CoreControlState>, request: web::Json<NeutralCalibrationRequest>) -> HttpResponse {
	match state.calibrate_neutral(request.valid_sample_count, request.pose.as_deref()).await {
		Ok(selection) => HttpResponse::Ok().json(ProfileDocumentResponse { selection }),
		Err(error) => HttpResponse::BadRequest().body(error.to_string()),
	}
}

#[post("/api/runtime/calibration/neutral/clear")]
async fn clear_neutral_calibration(state: web::Data<CoreControlState>) -> HttpResponse {
	match state.clear_neutral_calibration().await {
		Ok(selection) => HttpResponse::Ok().json(ProfileDocumentResponse { selection }),
		Err(error) => HttpResponse::BadRequest().body(error.to_string()),
	}
}

#[post("/api/runtime/face-pose-model/build")]
async fn build_face_pose_model(state: web::Data<CoreControlState>, request: web::Json<FacePoseModelBuildRequest>) -> HttpResponse {
	match state.build_face_pose_model(request.valid_sample_count).await {
		Ok(selection) => HttpResponse::Ok().json(ProfileDocumentResponse { selection }),
		Err(error) => HttpResponse::BadRequest().body(error.to_string()),
	}
}

#[get("/api/runtime/unmf/pose")]
async fn capture_unmotion_pose(state: web::Data<CoreControlState>) -> HttpResponse {
	match state.capture_unmotion_frame().await {
		Ok(frame) => HttpResponse::Ok().json(frame),
		Err(error) => HttpResponse::BadRequest().body(error.to_string()),
	}
}

#[post("/api/runtime/analysis-extras")]
async fn capture_analysis_extras(state: web::Data<CoreControlState>, request: web::Json<AnalysisExtrasCaptureRequest>) -> HttpResponse {
	match state.capture_analysis_extras(request.output_dir.clone(), request.duration_ms).await {
		Ok(samples) => HttpResponse::Ok().json(serde_json::json!({
			"schema": "un-motion.dev.analysis-extras.v1",
			"durationMs": request.duration_ms,
			"outputDir": request.output_dir,
			"samples": samples,
		})),
		Err(error) => HttpResponse::BadRequest().body(error.to_string()),
	}
}

#[get("/api/profiles")]
async fn profiles(state: web::Data<CoreControlState>) -> impl Responder {
	web::Json(SnapshotResponse {
		snapshot: state.snapshot().await,
	})
}

#[get("/api/profiles/document")]
async fn profile_document(state: web::Data<CoreControlState>) -> impl Responder {
	web::Json(ProfileDocumentResponse {
		selection: state.profile_document().await,
	})
}

#[put("/api/profiles/document")]
async fn set_profile_document(state: web::Data<CoreControlState>, request: web::Json<ProfileDocumentRequest>) -> HttpResponse {
	match state.set_profile_document(request.selection.clone()).await {
		Ok(selection) => HttpResponse::Ok().json(ProfileDocumentResponse { selection }),
		Err(error) => HttpResponse::InternalServerError().body(error.to_string()),
	}
}

#[put("/api/profiles/document/sync")]
async fn sync_profile_document(state: web::Data<CoreControlState>, request: web::Json<ProfileDocumentRequest>) -> HttpResponse {
	match state.sync_profile_document_preserving_active(request.selection.clone()).await {
		Ok(selection) => HttpResponse::Ok().json(ProfileDocumentResponse { selection }),
		Err(error) => HttpResponse::InternalServerError().body(error.to_string()),
	}
}

#[get("/api/profiles/active")]
async fn active_profile(state: web::Data<CoreControlState>) -> impl Responder {
	web::Json(StatusResponse {
		status: state.status().await,
	})
}

#[put("/api/profiles/active")]
async fn set_active_profile(state: web::Data<CoreControlState>, request: web::Json<ActiveProfileRequest>) -> HttpResponse {
	match state.set_active_profile(&request.profile_id).await {
		Ok(status) => HttpResponse::Ok().json(StatusResponse { status }),
		Err(error) => HttpResponse::NotFound().body(error.to_string()),
	}
}

#[get("/api/runtime/snapshot")]
async fn runtime_snapshot(state: web::Data<CoreControlState>) -> impl Responder {
	web::Json(SnapshotResponse {
		snapshot: state.snapshot().await,
	})
}

#[get("/api/telemetry/stream")]
async fn telemetry_stream(state: web::Data<CoreControlState>) -> HttpResponse {
	let samples = stream::unfold(
		(
			state.clone(),
			tokio::time::interval(Duration::from_millis(TELEMETRY_STREAM_INTERVAL_MS)),
		),
		|(state, mut interval)| async move {
			interval.tick().await;
			let snapshot = state.snapshot().await;
			Some((Ok::<Bytes, Error>(format_sse_json("telemetry", None, &snapshot)), (state, interval)))
		},
	);

	sse_response(samples)
}

#[get("/api/events")]
async fn events(state: web::Data<CoreControlState>) -> HttpResponse {
	events_response(state).await
}

#[get("/api/events/stream")]
async fn events_stream(state: web::Data<CoreControlState>) -> HttpResponse {
	events_response(state).await
}

async fn events_response(state: web::Data<CoreControlState>) -> HttpResponse {
	let initial_event = state.snapshot_event().await;
	let initial = stream::once(async move { Ok::<Bytes, Error>(format_sse_event(&initial_event)) });
	let event_stream = stream::unfold(state.subscribe(), |mut receiver| async move {
		loop {
			match receiver.recv().await {
				Ok(event) => return Some((Ok::<Bytes, Error>(format_sse_event(&event)), receiver)),
				Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
				Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
			}
		}
	});

	sse_response(initial.chain(event_stream))
}

fn sse_response<S>(body: S) -> HttpResponse
where
	S: futures_util::Stream<Item = Result<Bytes, Error>> + 'static,
{
	HttpResponse::Ok()
		.insert_header(("content-type", "text/event-stream"))
		.insert_header(("cache-control", "no-cache"))
		.streaming(body)
}

fn format_sse_event(event: &CoreEvent) -> Bytes {
	format_sse_json(event.kind.sse_name(), Some(event.sequence), event)
}

fn format_sse_json<T: Serialize>(event_name: &str, id: Option<u64>, payload: &T) -> Bytes {
	let payload = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string());
	let id = id.map(|id| format!("id: {id}\n")).unwrap_or_default();
	Bytes::from(format!("{id}event: {event_name}\ndata: {payload}\n\n"))
}

#[cfg(test)]
mod tests {
	use actix_web::{
		App,
		http::{StatusCode, header},
		test, web,
	};

	use super::*;

	#[actix_web::test]
	async fn rest_start_stop_status_endpoints_work() {
		let app = test::init_service(
			App::new()
				.app_data(web::Data::new(CoreControlState::default()))
				.configure(configure_api),
		)
		.await;

		let status: StatusResponse = test::call_and_read_body_json(&app, test::TestRequest::get().uri("/api/status").to_request()).await;
		assert!(!status.status.running);

		let started: StatusResponse =
			test::call_and_read_body_json(&app, test::TestRequest::post().uri("/api/runtime/start").to_request()).await;
		assert!(started.status.running);

		let stopped: StatusResponse =
			test::call_and_read_body_json(&app, test::TestRequest::post().uri("/api/runtime/stop").to_request()).await;
		assert!(!stopped.status.running);
	}

	#[actix_web::test]
	async fn cors_allows_tauri_webview_origin() {
		let app = test::init_service(
			App::new()
				.wrap(core_api_cors())
				.app_data(web::Data::new(CoreControlState::default()))
				.configure(configure_api),
		)
		.await;

		let response = test::call_service(
			&app,
			test::TestRequest::get()
				.uri("/api/status")
				.insert_header((header::ORIGIN, "tauri://localhost"))
				.to_request(),
		)
		.await;

		assert_eq!(response.status(), StatusCode::OK);
		assert_eq!(
			response
				.headers()
				.get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
				.and_then(|value| value.to_str().ok()),
			Some("tauri://localhost")
		);
	}

	#[actix_web::test]
	async fn cors_allows_loopback_dev_origin() {
		let app = test::init_service(
			App::new()
				.wrap(core_api_cors())
				.app_data(web::Data::new(CoreControlState::default()))
				.configure(configure_api),
		)
		.await;

		let response = test::call_service(
			&app,
			test::TestRequest::get()
				.uri("/api/status")
				.insert_header((header::ORIGIN, "http://localhost:1420"))
				.to_request(),
		)
		.await;

		assert_eq!(response.status(), StatusCode::OK);
		assert_eq!(
			response
				.headers()
				.get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
				.and_then(|value| value.to_str().ok()),
			Some("http://localhost:1420")
		);
	}

	#[actix_web::test]
	async fn cors_rejects_non_local_browser_origin() {
		let app = test::init_service(
			App::new()
				.wrap(core_api_cors())
				.app_data(web::Data::new(CoreControlState::default()))
				.configure(configure_api),
		)
		.await;

		let response = test::call_service(
			&app,
			test::TestRequest::get()
				.uri("/api/status")
				.insert_header((header::ORIGIN, "https://example.com"))
				.to_request(),
		)
		.await;

		assert_eq!(response.status(), StatusCode::OK);
		assert!(response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
	}

	#[actix_web::test]
	async fn active_profile_endpoint_rejects_unknown_profile() {
		let app = test::init_service(
			App::new()
				.app_data(web::Data::new(CoreControlState::default()))
				.configure(configure_api),
		)
		.await;
		let response = test::call_service(
			&app,
			test::TestRequest::put()
				.uri("/api/profiles/active")
				.set_json(ActiveProfileRequest {
					profile_id: "missing".to_string(),
				})
				.to_request(),
		)
		.await;
		assert_eq!(response.status(), StatusCode::NOT_FOUND);
	}

	#[actix_web::test]
	async fn profile_document_endpoint_roundtrips_selection() {
		let app = test::init_service(
			App::new()
				.app_data(web::Data::new(CoreControlState::default()))
				.configure(configure_api),
		)
		.await;
		let initial: ProfileDocumentResponse =
			test::call_and_read_body_json(&app, test::TestRequest::get().uri("/api/profiles/document").to_request()).await;
		assert_eq!(initial.selection.selected_profile_id, "default");

		let mut selection = initial.selection;
		selection.profiles[0].name = "Renamed".to_string();
		let updated: ProfileDocumentResponse = test::call_and_read_body_json(
			&app,
			test::TestRequest::put()
				.uri("/api/profiles/document")
				.set_json(ProfileDocumentRequest { selection })
				.to_request(),
		)
		.await;

		assert_eq!(updated.selection.profiles[0].name, "Renamed");
	}

	#[actix_web::test]
	async fn profile_document_sync_endpoint_preserves_active_selection() {
		let state = CoreControlState::new(vec![
			crate::CoreProfile {
				id: "p1".to_string(),
				name: "p1".to_string(),
				note: String::new(),
				icon_path: None,
				group: String::new(),
				engine: None,
			},
			crate::CoreProfile {
				id: "p2".to_string(),
				name: "p2".to_string(),
				note: String::new(),
				icon_path: None,
				group: String::new(),
				engine: None,
			},
		]);
		let app = test::init_service(App::new().app_data(web::Data::new(state)).configure(configure_api)).await;
		let mut initial: ProfileDocumentResponse =
			test::call_and_read_body_json(&app, test::TestRequest::get().uri("/api/profiles/document").to_request()).await;
		assert_eq!(initial.selection.selected_profile_id, "p1");

		initial.selection.selected_profile_id = "p2".to_string();
		initial.selection.profiles[1].name = "Changed p2".to_string();
		let updated: ProfileDocumentResponse = test::call_and_read_body_json(
			&app,
			test::TestRequest::put()
				.uri("/api/profiles/document/sync")
				.set_json(ProfileDocumentRequest {
					selection: initial.selection,
				})
				.to_request(),
		)
		.await;

		assert_eq!(updated.selection.selected_profile_id, "p1");
		assert_eq!(updated.selection.profiles[1].name, "Changed p2");
	}

	#[actix_web::test]
	async fn sse_event_format_contains_event_name_and_json_payload() {
		let state = CoreControlState::default();
		let event = state.snapshot_event().await;
		let formatted = String::from_utf8(format_sse_event(&event).to_vec()).expect("utf8");
		assert!(formatted.contains("event: snapshot"));
		assert!(formatted.contains("\"activeProfileId\":\"default\""));
	}

	#[actix_web::test]
	async fn runtime_snapshot_endpoint_returns_snapshot() {
		let app = test::init_service(
			App::new()
				.app_data(web::Data::new(CoreControlState::default()))
				.configure(configure_api),
		)
		.await;

		let response: SnapshotResponse =
			test::call_and_read_body_json(&app, test::TestRequest::get().uri("/api/runtime/snapshot").to_request()).await;

		assert_eq!(response.snapshot.status.active_profile_id, "default");
	}

	#[actix_web::test]
	async fn telemetry_stream_endpoint_is_sampled_sse() {
		let app = test::init_service(
			App::new()
				.app_data(web::Data::new(CoreControlState::default()))
				.configure(configure_api),
		)
		.await;

		let response = test::call_service(&app, test::TestRequest::get().uri("/api/telemetry/stream").to_request()).await;

		assert_eq!(response.status(), StatusCode::OK);
		assert_eq!(
			response.headers().get("content-type").and_then(|value| value.to_str().ok()),
			Some("text/event-stream")
		);
	}

	#[actix_web::test]
	async fn telemetry_sse_payload_contains_snapshot_without_event_sequence() {
		let state = CoreControlState::default();
		let formatted = String::from_utf8(format_sse_json("telemetry", None, &state.snapshot().await).to_vec()).expect("utf8");

		assert!(!formatted.contains("id: "));
		assert!(formatted.contains("event: telemetry"));
		assert!(formatted.contains("\"activeProfileId\":\"default\""));
	}
}
