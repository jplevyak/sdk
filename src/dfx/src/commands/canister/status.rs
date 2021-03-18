use crate::lib::environment::Environment;
use crate::lib::error::DfxResult;
use crate::lib::identity::identity_utils::CallSender;
use crate::lib::models::canister_id_store::CanisterIdStore;
use crate::lib::operations::canister;
use crate::lib::root_key::fetch_root_key_if_needed;
use crate::util::expiry_duration;

use anyhow::bail;
use clap::Clap;
use slog::info;
use std::time::Duration;

/// Returns the current status of the canister on the Internet Computer network: Running, Stopping, or Stopped.
#[derive(Clap)]
pub struct CanisterStatusOpts {
    /// Specifies the name of the canister to return information for.
    /// You must specify either a canister name or the --all flag.
    canister_name: Option<String>,

    /// Returns status information for all of the canisters configured in the dfx.json file.
    #[clap(long, required_unless_present("canister-name"))]
    all: bool,
}

async fn canister_status(
    env: &dyn Environment,
    canister_name: &str,
    timeout: Duration,
    call_sender: &CallSender,
) -> DfxResult {
    let log = env.get_logger();
    let canister_id_store = CanisterIdStore::for_env(env)?;
    let canister_id = canister_id_store.get(canister_name)?;

    let status = canister::get_canister_status(env, canister_id, timeout, call_sender).await?;

    info!(log, "Canister status call result for {}.\nStatus: {}\nController: {}\nMemory Size: {:?}\nBalance: {} Cycles",
        canister_name,
        status.status,
        status.controller.to_text(),
        status.memory_size,
        status.cycles,
    );
    Ok(())
}

pub async fn exec(
    env: &dyn Environment,
    opts: CanisterStatusOpts,
    call_sender: &CallSender,
) -> DfxResult {
    let config = env.get_config_or_anyhow()?;

    fetch_root_key_if_needed(env).await?;
    let timeout = expiry_duration();

    if let Some(canister_name) = opts.canister_name.as_deref() {
        canister_status(env, &canister_name, timeout, call_sender).await
    } else if opts.all {
        if let Some(canisters) = &config.get_config().canisters {
            for canister_name in canisters.keys() {
                canister_status(env, &canister_name, timeout, call_sender).await?;
            }
        }
        Ok(())
    } else {
        bail!("Cannot find canister name.")
    }
}
