use crate::lib::environment::Environment;
use crate::lib::error::DfxResult;
use crate::lib::ic_attributes::CanisterSettings;
use crate::lib::identity::identity_utils::CallSender;
use crate::lib::models::canister_id_store::CanisterIdStore;
use crate::lib::operations::canister;
use crate::lib::operations::canister::{deposit_cycles, stop_canister, update_settings};
use crate::lib::root_key::fetch_root_key_if_needed;
use crate::lib::waiter::waiter_with_timeout;
use crate::util::assets::wallet_wasm;
use crate::util::{blob_from_arguments, expiry_duration};
use ic_utils::call::AsyncCall;
use ic_utils::interfaces::management_canister::attributes::{
    ComputeAllocation, FreezingThreshold, MemoryAllocation,
};
use ic_utils::interfaces::management_canister::CanisterStatus;

use anyhow::anyhow;
use clap::Clap;
use ic_types::Principal;
use ic_utils::interfaces::management_canister::builders::InstallMode;
use ic_utils::interfaces::ManagementCanister;
use num_traits::cast::ToPrimitive;
use slog::info;
use std::convert::TryFrom;
use std::time::Duration;

const WITHDRAWL_COST: u64 = 10_000_000_000; // Emperically estimateed ~2B.

/// Deletes a canister on the Internet Computer network.
#[derive(Clap)]
pub struct CanisterDeleteOpts {
    /// Specifies the name of the canister to delete.
    /// You must specify either a canister name/id or the --all flag.
    canister: Option<String>,

    /// Deletes all of the canisters configured in the dfx.json file.
    #[clap(long, required_unless_present("canister"))]
    all: bool,

    /// Withdraw cycles from canister(s) to wallet before deleting.
    #[clap(long)]
    withdraw_cycles_to_wallet: Option<String>,
}

async fn delete_canister(
    env: &dyn Environment,
    canister: &str,
    timeout: Duration,
    call_sender: &CallSender,
    withdraw_cycles_to_wallet: Option<String>,
) -> DfxResult {
    let log = env.get_logger();
    let mut canister_id_store = CanisterIdStore::for_env(env)?;
    let canister_id =
        Principal::from_text(canister).or_else(|_| canister_id_store.get(canister))?;
    if let Some(target_wallet_canister_id) = withdraw_cycles_to_wallet {
        // Get the wallet to transfer the cycles to.
        let target_wallet_canister_id = Principal::from_text(target_wallet_canister_id)?;
        fetch_root_key_if_needed(env).await?;

        // Determine how many cycles we can withdraw.
        let status = canister::get_canister_status(env, canister_id, timeout, call_sender).await?;
        let mut cycles = status.cycles.0.to_u64().unwrap();
        if status.status != CanisterStatus::Stopped || cycles > WITHDRAWL_COST {
            cycles -= WITHDRAWL_COST;
            info!(
                log,
                "Beginning withdrawl of {} cycles to wallet {}.", cycles, target_wallet_canister_id
            );

            let agent = env
                .get_agent()
                .ok_or_else(|| anyhow!("Cannot get HTTP client from environment."))?;
            let mgr = ManagementCanister::create(agent);
            let canister_id =
                Principal::from_text(canister).or_else(|_| canister_id_store.get(canister))?;
            let principal = env
                .get_selected_identity_principal()
                .expect("Selected identity not instantiated.");

            // Set this principal to be a controller and default the other settings.
            let settings = CanisterSettings {
                controller: Some(principal),
                compute_allocation: Some(ComputeAllocation::try_from(0).unwrap()),
                memory_allocation: Some(MemoryAllocation::try_from(0).unwrap()),
                freezing_threshold: Some(FreezingThreshold::try_from(2592000).unwrap()),
            };
            info!(log, "Setting the controller to identity princpal.");
            update_settings(env, canister_id, settings, timeout, call_sender).await?;

            // Install a temporary wallet wasm which will transfer the cycles out of the canister before it is deleted.
            let wasm_module = wallet_wasm(env.get_logger())?;
            info!(
                log,
                "Installing temporary wallet in canister {} to enable transfer of cycles.",
                canister
            );
            let args = blob_from_arguments(None, None, None, &None)?;
            let mode = InstallMode::Reinstall;
            let install_builder = mgr
                .install_code(&canister_id, &wasm_module)
                .with_raw_arg(args.to_vec())
                .with_mode(mode);
            install_builder
                .build()?
                .call_and_wait(waiter_with_timeout(timeout))
                .await?;

            // Transfer cycles from the canister to the regular wallet using the temporary wallet.
            info!(log, "Transfering cycles.");
            deposit_cycles(
                env,
                target_wallet_canister_id,
                timeout,
                &CallSender::Wallet(canister_id),
                cycles,
            )
            .await?;
            stop_canister(env, canister_id, timeout, &CallSender::SelectedId).await?;
        } else if status.status != CanisterStatus::Stopped {
            info!(
                log,
                "Canister {} must be stopped before it is deleted.", canister_id
            );
        } else {
            info!(log, "Too few cycles to withdraw: {}.", cycles);
        }
    }

    info!(
        log,
        "Deleting code for canister {}, with canister_id {}",
        canister,
        canister_id.to_text(),
    );

    canister::delete_canister(env, canister_id, timeout, &call_sender).await?;

    canister_id_store.remove(canister)?;

    Ok(())
}

pub async fn exec(
    env: &dyn Environment,
    opts: CanisterDeleteOpts,
    call_sender: &CallSender,
) -> DfxResult {
    let config = env.get_config_or_anyhow()?;
    let timeout = expiry_duration();

    fetch_root_key_if_needed(env).await?;

    if let Some(canister) = opts.canister.as_deref() {
        delete_canister(
            env,
            canister,
            timeout,
            call_sender,
            opts.withdraw_cycles_to_wallet,
        )
        .await
    } else if opts.all {
        if let Some(canisters) = &config.get_config().canisters {
            for canister in canisters.keys() {
                delete_canister(
                    env,
                    canister,
                    timeout,
                    call_sender,
                    opts.withdraw_cycles_to_wallet.clone(),
                )
                .await?;
            }
        }
        Ok(())
    } else {
        unreachable!()
    }
}
