use std::path::PathBuf;

use crate::{CoreApiConfig, CoreControlState, run_api_server};

/// System tray アイコン + コンテキストメニューの設定。
///
/// UN Motion Core / Capturer の tray 実装。tooltip / 開きたい exe /
/// メニューラベルを呼び出し側から差し込めるようにする。
///
/// Capturer 側は:
/// - tooltip = "UN Motion Capturer (PID xxxxx)"
/// - open_menu_label = "Open Supervisor Console"
/// - exit_menu_label = "Exit Capturer (PID xxxxx)"
/// - open_exe = `un-motion-supervisor.exe`
///
/// として使うことで「Supervisor を閉じても Capturer は tray から見える / 終了できる」
/// 運用に対応する。
#[derive(Clone, Debug, Default)]
pub struct TrayOptions {
	/// マウスホバー時に表示する tooltip 文字列。
	pub tooltip: Option<String>,
	/// "Open ..." メニューから起動する exe path (Supervisor など)。`None` の場合は
	/// メニュー項目は無効化される。
	pub open_exe: Option<PathBuf>,
	/// "Open ..." メニューに表示するラベル。`None` の場合は既定 ("Open Supervisor Console")。
	pub open_menu_label: Option<String>,
	/// "Exit ..." メニューに表示するラベル。`None` の場合は既定 ("Exit UNMotion Core")。
	pub exit_menu_label: Option<String>,
	/// 起動時に open_exe を自動 spawn するかどうか。Capturer 系は基本 `false`。
	pub open_on_startup: Option<bool>,
}

#[cfg(target_os = "windows")]
pub fn run_core_with_tray(config: CoreApiConfig, state: CoreControlState, options: TrayOptions) -> anyhow::Result<()> {
	windows_tray::run(config, state, options)
}

#[cfg(not(target_os = "windows"))]
pub fn run_core_with_tray(_config: CoreApiConfig, _state: CoreControlState, _options: TrayOptions) -> anyhow::Result<()> {
	anyhow::bail!("core tray mode is currently implemented for Windows only")
}

#[cfg(target_os = "windows")]
mod windows_tray {
	use std::net::{SocketAddr, TcpStream};
	use std::os::windows::io::AsRawHandle;
	use std::path::Path;
	use std::process::Child;
	use std::sync::Arc;
	use std::thread;
	use std::time::{Duration, Instant};

	use tao::event::Event;
	use tao::event_loop::{ControlFlow, EventLoopBuilder};
	use tray_icon::{
		Icon, TrayIconBuilder, TrayIconEvent,
		menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
	};
	use windows::Win32::{
		Foundation::{CloseHandle, HANDLE},
		System::JobObjects::{
			AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
			JobObjectExtendedLimitInformation, SetInformationJobObject,
		},
	};

	use super::*;

	const OPEN_MENU_ID: &str = "open-desktop";
	const EXIT_MENU_ID: &str = "exit-core";

	#[derive(Debug)]
	enum TrayEvent {
		Menu(MenuEvent),
		Tray(TrayIconEvent),
	}

	pub fn run(config: CoreApiConfig, state: CoreControlState, options: super::TrayOptions) -> anyhow::Result<()> {
		config.validate()?;
		let open_exe = options.open_exe.clone();
		let bind_addr = config.bind_addr;
		let open_job = Arc::new(OpenProcessJob::new()?);
		let tooltip = options.tooltip.clone().unwrap_or_else(|| "UNMotion Core".to_string());
		let open_label = options
			.open_menu_label
			.clone()
			.unwrap_or_else(|| "Open Supervisor Console".to_string());
		let exit_label = options.exit_menu_label.clone().unwrap_or_else(|| "Exit UNMotion Core".to_string());
		let open_on_startup = options.open_on_startup.unwrap_or(false);
		spawn_api_server(config, state);

		let event_loop = EventLoopBuilder::<TrayEvent>::with_user_event().build();
		let proxy = event_loop.create_proxy();
		TrayIconEvent::set_event_handler(Some(move |event| {
			let _ = proxy.send_event(TrayEvent::Tray(event));
		}));
		let proxy = event_loop.create_proxy();
		MenuEvent::set_event_handler(Some(move |event| {
			let _ = proxy.send_event(TrayEvent::Menu(event));
		}));

		let menu = Menu::new();
		let open_item = MenuItem::with_id(OPEN_MENU_ID, &open_label, open_exe.is_some(), None);
		let separator = PredefinedMenuItem::separator();
		let exit_item = MenuItem::with_id(EXIT_MENU_ID, &exit_label, true, None);
		menu.append(&open_item)?;
		menu.append(&separator)?;
		menu.append(&exit_item)?;

		let tray_icon = TrayIconBuilder::new()
			.with_tooltip(&tooltip)
			.with_icon(unmotion_tray_icon()?)
			.with_menu(Box::new(menu.clone()))
			.build()?;
		if open_on_startup {
			wait_for_api(bind_addr, Duration::from_secs(3));
			launch_open_exe_if_configured(open_exe.as_deref(), &open_job);
		}

		let open_id = MenuId::new(OPEN_MENU_ID);
		let exit_id = MenuId::new(EXIT_MENU_ID);
		event_loop.run(move |event, _target, control_flow| {
			let _keep_alive = (&menu, &open_item, &separator, &exit_item, &tray_icon);
			*control_flow = ControlFlow::Wait;
			match event {
				Event::UserEvent(TrayEvent::Menu(event)) if event.id == open_id => {
					launch_open_exe_if_configured(open_exe.as_deref(), &open_job);
				}
				Event::UserEvent(TrayEvent::Menu(event)) if event.id == exit_id => {
					*control_flow = ControlFlow::Exit;
				}
				Event::UserEvent(TrayEvent::Tray(TrayIconEvent::DoubleClick { .. })) => {
					launch_open_exe_if_configured(open_exe.as_deref(), &open_job);
				}
				_ => {}
			}
		});
	}

	fn spawn_api_server(config: CoreApiConfig, state: CoreControlState) {
		thread::spawn(move || {
			let result = actix_web::rt::System::new().block_on(run_api_server(config, state));
			if let Err(error) = result {
				eprintln!("un-motion-core API server stopped: {error}");
				std::process::exit(1);
			}
		});
	}

	fn launch_open_exe_if_configured(open_exe: Option<&Path>, open_job: &OpenProcessJob) {
		let Some(open_exe) = open_exe else {
			return;
		};
		match spawn_open_process(open_exe, |child| open_job.assign(child)) {
			Ok(_) => {}
			Err(error) => {
				eprintln!("failed to launch tray target: {error}");
			}
		}
	}

	fn spawn_open_process(open_exe: &Path, setup_child: impl FnOnce(&Child) -> anyhow::Result<()>) -> anyhow::Result<u32> {
		let mut child = std::process::Command::new(open_exe).spawn()?;
		if let Err(error) = setup_child(&child) {
			let _ = child.kill();
			let _ = child.wait();
			return Err(error);
		}
		Ok(child.id())
	}

	struct OpenProcessJob {
		handle: HANDLE,
	}

	impl OpenProcessJob {
		fn new() -> anyhow::Result<Self> {
			let handle = unsafe { CreateJobObjectW(None, None)? };
			let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
			info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
			unsafe {
				SetInformationJobObject(
					handle,
					JobObjectExtendedLimitInformation,
					&mut info as *mut _ as *const _,
					std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
				)?;
			}
			Ok(Self { handle })
		}

		fn assign(&self, child: &Child) -> anyhow::Result<()> {
			let process = HANDLE(child.as_raw_handle());
			unsafe { AssignProcessToJobObject(self.handle, process)? };
			Ok(())
		}
	}

	impl Drop for OpenProcessJob {
		fn drop(&mut self) {
			let _ = unsafe { CloseHandle(self.handle) };
		}
	}

	fn wait_for_api(addr: SocketAddr, timeout: Duration) {
		let started = Instant::now();
		while started.elapsed() < timeout {
			if TcpStream::connect_timeout(&addr, Duration::from_millis(120)).is_ok() {
				return;
			}
			thread::sleep(Duration::from_millis(50));
		}
	}

	fn unmotion_tray_icon() -> anyhow::Result<Icon> {
		let image = image::load_from_memory(include_bytes!("../../../assets/brand/un-motion-artwork-capturer.png"))?
			.resize_exact(32, 32, image::imageops::FilterType::Lanczos3)
			.into_rgba8();
		Icon::from_rgba(image.into_raw(), 32, 32).map_err(Into::into)
	}
}
