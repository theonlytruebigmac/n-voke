//! `nvoke`: the terminal interface for N-voke. Shares its API key, settings and
//! history with the desktop app through `nvoke-core`.

mod app;
mod input;
mod ui;

use std::{error::Error, sync::Arc};

use nvoke_core::Core;

fn main() -> Result<(), Box<dyn Error>> {
    // The core is async (HTTP, keychain), so the app owns a runtime and hands
    // work to it; the terminal loop itself stays an ordinary blocking loop.
    let runtime = tokio::runtime::Runtime::new()?;
    let core = Arc::new(Core::new(env!("CARGO_PKG_VERSION"))?);

    // `init` switches to the alternate screen and installs a panic hook that
    // restores the terminal, so a crash doesn't leave it in raw mode.
    let mut terminal = ratatui::init();
    let result = app::App::new(core, runtime.handle().clone()).run(&mut terminal);
    ratatui::restore();
    Ok(result?)
}
