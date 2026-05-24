use un_motion_core::{CoreApiConfig, CoreControlState, TrayOptions, run_api_server, run_core_with_tray};

fn main() -> anyhow::Result<()> {
	let config = CoreApiConfig::from_args(std::env::args().skip(1))?;
	let state = CoreControlState::from_workspace()?;
	eprintln!("unmotion-core API listening on http://{}", config.bind_addr);
	if config.tray_enabled {
		run_core_with_tray(config, state, TrayOptions::default())?;
	} else {
		actix_web::rt::System::new().block_on(run_api_server(config, state))?;
	}
	Ok(())
}
