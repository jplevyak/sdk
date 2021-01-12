use crate::lib::config::get_config_dfx_dir_path;
use crate::lib::environment::Environment;
use crate::lib::error::{DfxError, DfxResult, IdentityError};
use crate::lib::identity::Identity;

use anyhow::Context;
use pem::{encode, Pem};
use ring::{rand, signature};
use serde::{Deserialize, Serialize};
use slog::Logger;
use std::boxed::Box;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_IDENTITY_NAME: &str = "default";
const ANONYMOUS_IDENTITY_NAME: &str = "anonymous";

/// TODO: move this to identity/mod.rs
const IDENTITY_PEM: &str = "identity.pem";
const IDENTITY_JSON: &str = "identity.json";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Configuration {
    #[serde(default = "default_identity")]
    pub default: String,
}

fn default_identity() -> String {
    String::from(DEFAULT_IDENTITY_NAME)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IdentityConfiguration {
    pub hsm: Option<HardwareIdentityConfiguration>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HardwareIdentityConfiguration {
    /// The file path to the opensc-pkcs11 library e.g. "/usr/local/lib/opensc-pkcs11.so"
    pub pkcs11_lib_path: String,

    /// A sequence of pairs of hex digits
    pub key_id: String,
}

pub enum IdentityCreationParameters {
    Pem(),

    Hardware(HardwareIdentityConfiguration),
}

#[derive(Clone, Debug)]
pub struct IdentityManager {
    identity_json_path: PathBuf,
    identity_root_path: PathBuf,
    configuration: Configuration,
    selected_identity: String,
}

impl IdentityManager {
    pub fn new(env: &dyn Environment) -> DfxResult<Self> {
        let config_dfx_dir_path = get_config_dfx_dir_path()?;
        let identity_root_path = config_dfx_dir_path.join("identity");
        let identity_json_path = config_dfx_dir_path.join("identity.json");

        let configuration = if identity_json_path.exists() {
            read_configuration(&identity_json_path)
        } else {
            initialize(env.get_logger(), &identity_json_path, &identity_root_path)
        }?;

        let identity_override = env.get_identity_override();
        let selected_identity = identity_override
            .clone()
            .unwrap_or_else(|| configuration.default.clone());

        let mgr = IdentityManager {
            identity_json_path,
            identity_root_path,
            configuration,
            selected_identity,
        };

        if let Some(identity) = identity_override {
            mgr.require_identity_exists(&identity)?;
        }

        Ok(mgr)
    }

    /// Create an Identity instance for use with an Agent
    pub fn instantiate_selected_identity(&self) -> DfxResult<Box<Identity>> {
        self.instantiate_identity_from_name(self.selected_identity.as_str())
    }

    /// Provide a valid Identity name and create its Identity instance for use with an Agent
    pub fn instantiate_identity_from_name(&self, identity_name: &str) -> DfxResult<Box<Identity>> {
        self.require_identity_exists(identity_name)?;
        Ok(Box::new(Identity::load(self, identity_name)?))
    }

    /// Create a new identity (name -> generated key)
    pub fn create_new_identity(
        &self,
        name: &str,
        parameters: IdentityCreationParameters,
    ) -> DfxResult {
        if name == ANONYMOUS_IDENTITY_NAME {
            return Err(DfxError::new(IdentityError::CannotCreateAnonymousIdentity()));
        }

        let _ = Identity::create(self, name, parameters)?;
        Ok(())
    }

    /// Return a sorted list of all available identity names
    pub fn get_identity_names(&self) -> DfxResult<Vec<String>> {
        let mut names = self
            .identity_root_path
            .read_dir()?
            .filter(|entry_result| match entry_result {
                Ok(dir_entry) => match dir_entry.file_type() {
                    Ok(file_type) => file_type.is_dir(),
                    _ => false,
                },
                _ => false,
            })
            .map(|entry_result| {
                entry_result.map(|entry| entry.file_name().to_string_lossy().to_string())
            })
            .collect::<Result<Vec<_>, std::io::Error>>()?;

        names.sort();

        Ok(names)
    }

    /// Return the name of the currently selected (active) identity
    pub fn get_selected_identity_name(&self) -> &String {
        &self.selected_identity
    }

    /// Remove a named identity.
    /// Removing the selected identity is not allowed.
    pub fn remove(&self, name: &str) -> DfxResult {
        self.require_identity_exists(name)?;

        if self.configuration.default == name {
            return Err(DfxError::new(IdentityError::CannotDeleteDefaultIdentity()));
        }

        remove_identity_file(&self.get_identity_json_path(name))?;
        remove_identity_file(&self.get_identity_pem_path(name))?;

        let dir = self.get_identity_dir_path(name);
        std::fs::remove_dir(&dir).context(format!(
            "Cannot remove identity directroy at '{}'.",
            dir.display()
        ))?;

        Ok(())
    }

    /// Rename an identity.
    /// If renaming the selected (default) identity, changes that
    /// to refer to the new identity name.
    pub fn rename(&self, from: &str, to: &str) -> DfxResult<bool> {
        if to == ANONYMOUS_IDENTITY_NAME {
            return Err(DfxError::new(IdentityError::CannotCreateAnonymousIdentity()));
        }
        self.require_identity_exists(from)?;

        let from_id = self.instantiate_identity_from_name(from)?;
        let to_id = self.instantiate_identity_from_name(to)?;

        let from_dir = from_id.dir;
        let to_dir = to_id.dir;

        if to_dir.exists() {
            return Err(DfxError::new(IdentityError::IdentityAlreadyExists()));
        }

        std::fs::rename(&from_dir, &to_dir).map_err(|err| {
            DfxError::new(IdentityError::CannotRenameIdentityDirectory(
                from_dir,
                to_dir,
                Box::new(DfxError::new(err)),
            ))
        })?;

        if from == self.configuration.default {
            self.write_default_identity(to)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Select an identity by name to use by default
    pub fn use_identity_named(&self, name: &str) -> DfxResult {
        self.require_identity_exists(name)?;
        self.write_default_identity(name)
    }

    fn write_default_identity(&self, name: &str) -> DfxResult {
        let config = Configuration {
            default: String::from(name),
        };
        write_configuration(&self.identity_json_path, &config)
    }

    fn require_identity_exists(&self, name: &str) -> DfxResult {
        let identity_pem_path = self.get_identity_pem_path(name);

        if !identity_pem_path.exists() {
            let identity_json_path = self.get_identity_json_path(name);
            if !identity_json_path.exists() {
                Err(DfxError::new(IdentityError::IdentityDoesNotExist(
                    String::from(name),
                    identity_pem_path,
                )))
            } else {
                Ok(())
            }
        } else {
            Ok(())
        }
    }

    pub fn get_identity_dir_path(&self, identity: &str) -> PathBuf {
        self.identity_root_path.join(&identity)
    }

    pub fn get_identity_pem_path(&self, identity: &str) -> PathBuf {
        self.get_identity_dir_path(identity).join(IDENTITY_PEM)
    }

    pub fn get_identity_json_path(&self, identity: &str) -> PathBuf {
        self.get_identity_dir_path(identity).join(IDENTITY_JSON)
    }
}

pub(super) fn get_dfx_hsm_pin() -> Result<String, String> {
    std::env::var("DFX_HSM_PIN")
        .map_err(|_| "There is no DFX_HSM_PIN environment variable.".to_string())
}

fn initialize(
    logger: &Logger,
    identity_json_path: &Path,
    identity_root_path: &Path,
) -> DfxResult<Configuration> {
    slog::info!(logger, r#"Creating the "default" identity."#);

    let identity_dir = identity_root_path.join(DEFAULT_IDENTITY_NAME);
    let identity_pem_path = identity_dir.join(IDENTITY_PEM);
    if !identity_pem_path.exists() {
        if !identity_dir.exists() {
            std::fs::create_dir_all(&identity_dir).map_err(|err| {
                DfxError::new(IdentityError::CannotCreateIdentityDirectory(
                    identity_dir,
                    Box::new(DfxError::new(err)),
                ))
            })?;
        }

        let creds_pem_path = get_legacy_creds_pem_path()?;
        if creds_pem_path.exists() {
            slog::info!(
                logger,
                "  - migrating key from {} to {}",
                creds_pem_path.display(),
                identity_pem_path.display()
            );
            fs::copy(creds_pem_path, identity_pem_path)?;
        } else {
            slog::info!(
                logger,
                "  - generating new key at {}",
                identity_pem_path.display()
            );
            generate_key(&identity_pem_path)?;
        }
    } else {
        slog::info!(
            logger,
            "  - using key already in place at {}",
            identity_pem_path.display()
        );
    }

    let config = Configuration {
        default: String::from(DEFAULT_IDENTITY_NAME),
    };
    write_configuration(&identity_json_path, &config)?;
    slog::info!(logger, r#"Created the "default" identity."#);

    Ok(config)
}

fn get_legacy_creds_pem_path() -> DfxResult<PathBuf> {
    let home = std::env::var("HOME")
        .map_err(|_| DfxError::new(IdentityError::CannotFindHomeDirectory()))?;

    Ok(PathBuf::from(home)
        .join(".dfinity")
        .join("identity")
        .join("creds.pem"))
}

fn read_configuration(path: &Path) -> DfxResult<Configuration> {
    let content = std::fs::read_to_string(&path).context(format!(
        "Cannot read configuration file at '{}'.",
        PathBuf::from(path).display()
    ))?;
    serde_json::from_str(&content).map_err(DfxError::from)
}

fn write_configuration(path: &Path, config: &Configuration) -> DfxResult {
    let content = serde_json::to_string_pretty(&config)?;
    std::fs::write(&path, content).context(format!(
        "Cannot write configuration file at '{}'.",
        PathBuf::from(path).display()
    ))?;
    Ok(())
}

pub(super) fn read_identity_configuration(path: &Path) -> DfxResult<IdentityConfiguration> {
    let content = std::fs::read_to_string(&path).context(format!(
        "Cannot read identity configuration file at '{}'.",
        PathBuf::from(path).display()
    ))?;
    serde_json::from_str(&content).map_err(DfxError::from)
}

pub(super) fn write_identity_configuration(
    path: &Path,
    config: &IdentityConfiguration,
) -> DfxResult {
    let content = serde_json::to_string_pretty(&config)?;
    std::fs::write(&path, content).context(format!(
        "Cannot write identity configuration file at '{}'.",
        PathBuf::from(path).display()
    ))?;
    Ok(())
}

fn remove_identity_file(file: &Path) -> DfxResult {
    if file.exists() {
        std::fs::remove_file(&file).context(format!(
            "Cannot remove identity file at '{}'.",
            file.display()
        ))?;
    }
    Ok(())
}

pub(super) fn generate_key(pem_file: &Path) -> DfxResult {
    let rng = rand::SystemRandom::new();
    let pkcs8_bytes = signature::Ed25519KeyPair::generate_pkcs8(&rng)
        .map_err(|x| DfxError::new(IdentityError::CannotGenerateKeyPair(x)))?;

    let encoded_pem = encode_pem_private_key(&(*pkcs8_bytes.as_ref()));
    fs::write(&pem_file, encoded_pem)?;

    let mut permissions = fs::metadata(&pem_file)?.permissions();
    permissions.set_readonly(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o400);
    }

    fs::set_permissions(&pem_file, permissions)?;

    Ok(())
}

fn encode_pem_private_key(key: &[u8]) -> String {
    let pem = Pem {
        tag: "PRIVATE KEY".to_owned(),
        contents: key.to_vec(),
    };
    encode(&pem)
}
