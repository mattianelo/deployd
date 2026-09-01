use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::core::game::{self, WineConfig};
use crate::dlog;
use crate::models::game::Game;
use crate::models::tool::Tool;
use crate::utils::paths;

use super::runtime;
use super::{ToolPrepareError, ToolSetupStage};

const PREFIX_MARKER: &str = ".deployd_snap_tool_prefix_v1";
const MONO_MARKER: &str = ".deployd_wine_mono_10_4_1";
const MONO_VERSION: &str = "10.4.1";
const MONO_FILE_NAME: &str = "wine-mono-10.4.1-x86.msi";
const MONO_URL: &str = "https://dl.winehq.org/wine/wine-mono/10.4.1/wine-mono-10.4.1-x86.msi";
const MONO_SIZE: u64 = 85_504_000;
const MONO_DOWNLOAD_LIMIT: u64 = 90_000_000;
const MONO_SHA256: &str = "071f4b2887e1c97a11d791ff3d65be9429eed6dec4c2708888bfd546ba358e23";
const PREFIX_INIT_DLL_OVERRIDES: &str = "mscoree=d;mshtml=d;winemenubuilder.exe=d";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CachedPackageAction {
    Reuse,
    Replace,
}

pub(super) fn initial_setup_required(wine_config: &WineConfig) -> bool {
    !prefix_is_initialized(&wine_config.prefix)
}

pub(super) fn build_command(
    wine_binary: &Path,
    wine_platform: &Path,
    wine_runtime: &Path,
    tool: &Tool,
    game: &Game,
    wine_config: &WineConfig,
) -> Command {
    let mut command = setup_command(wine_config, wine_binary, wine_platform, wine_runtime);
    command.arg(super::effective_tool_exe_path(tool));
    for argument in tool.custom_args.split_whitespace() {
        command.arg(argument);
    }
    command.current_dir(super::effective_cwd(tool, game));
    command
}

pub(super) async fn prepare_runtime(
    game: &Game,
    wine_config: &WineConfig,
    cancel: Arc<AtomicBool>,
    skip_mono: bool,
    on_progress: Arc<dyn Fn(ToolSetupStage) + Send + Sync>,
) -> std::result::Result<(), ToolPrepareError> {
    let game = game.clone();
    let config = wine_config.clone();
    let init_cancel = cancel.clone();
    let init_progress = on_progress.clone();
    tokio::task::spawn_blocking(move || {
        initialize_prefix(&game, &config, init_cancel.as_ref(), init_progress)
    })
    .await
    .map_err(|error| ToolPrepareError::Fatal(format!("Wine prefix setup stopped: {error}")))?
    .map_err(|error| prepare_error(error, cancel.as_ref()))?;

    if should_prepare_mono(skip_mono) {
        prepare_mono(wine_config, cancel.clone(), on_progress.clone()).await?;
    }
    on_progress(ToolSetupStage::LaunchingTool);
    Ok(())
}

fn should_prepare_mono(skip_mono: bool) -> bool {
    !skip_mono
}

fn prepare_error(error: anyhow::Error, cancel: &AtomicBool) -> ToolPrepareError {
    if cancel.load(std::sync::atomic::Ordering::SeqCst) {
        ToolPrepareError::Cancelled
    } else {
        ToolPrepareError::Fatal(error.to_string())
    }
}

fn initialize_prefix(
    game: &Game,
    wine_config: &WineConfig,
    cancel: &AtomicBool,
    on_progress: Arc<dyn Fn(ToolSetupStage) + Send + Sync>,
) -> Result<()> {
    let game_prefix = game
        .wine_prefix
        .as_deref()
        .ok_or_else(|| anyhow!("The game has no configured Wine prefix"))?;
    if game_prefix == wine_config.prefix {
        return Err(anyhow!(
            "Snap tool setup refused to modify the configured game prefix"
        ));
    }

    let final_prefix = &wine_config.prefix;
    if prefix_is_initialized(final_prefix) {
        on_progress(ToolSetupStage::ConfiguringPrefix);
        verify_source_bridges(game_prefix, final_prefix)?;
        ensure_game_drive_mappings(game_prefix, final_prefix, &game.path)?;
        configure_prefix(game, wine_config, cancel)?;
        return Ok(());
    }
    if final_prefix.exists() {
        return Err(anyhow!(
            "The Snap tool prefix is incomplete. Remove it from Deployd's app data and retry"
        ));
    }

    on_progress(ToolSetupStage::CreatingPrefix);
    let parent = final_prefix
        .parent()
        .ok_or_else(|| anyhow!("Cannot resolve the Snap tool prefix parent"))?;
    std::fs::create_dir_all(parent).context("create Snap Wine prefix storage")?;
    let temporary = tempfile::Builder::new()
        .prefix(".prefix-setup-")
        .tempdir_in(parent)
        .context("create temporary Snap Wine prefix")?;
    let temporary_prefix = temporary.path().to_path_buf();
    let temporary_config = WineConfig {
        prefix: temporary_prefix.clone(),
        launcher: wine_config.launcher.clone(),
    };

    let mut wineboot = prefix_init_command(&temporary_config)?;
    run_required(
        &mut wineboot,
        Some(cancel),
        "initialize the Snap Wine prefix",
    )?;

    on_progress(ToolSetupStage::ConfiguringPrefix);
    create_source_bridges(game_prefix, &temporary_prefix)?;
    ensure_game_drive_mappings(game_prefix, &temporary_prefix, &game.path)?;
    configure_prefix(game, &temporary_config, cancel)?;
    std::fs::write(temporary_prefix.join(PREFIX_MARKER), b"1")
        .context("record completed Snap Wine prefix setup")?;

    std::fs::rename(temporary.path(), final_prefix).with_context(|| {
        format!(
            "publish the prepared Snap Wine prefix at {}",
            final_prefix.display()
        )
    })?;
    Ok(())
}

fn prefix_is_initialized(prefix: &Path) -> bool {
    prefix.join(PREFIX_MARKER).is_file()
        && prefix.join("system.reg").is_file()
        && prefix.join("dosdevices/c:").symlink_metadata().is_ok()
}

fn create_source_bridges(source_prefix: &Path, tool_prefix: &Path) -> Result<()> {
    let source_user = prefix_user_dir(source_prefix)?;
    let tool_user = prefix_user_dir(tool_prefix)?;
    for relative in ["Documents", "AppData"] {
        let source = source_user.join(relative);
        std::fs::create_dir_all(&source)
            .with_context(|| format!("create source Wine {relative} directory"))?;
        let destination = tool_user.join(relative);
        if destination.symlink_metadata().is_ok() {
            if destination.is_dir() && !destination.is_symlink() {
                std::fs::remove_dir_all(&destination)
                    .with_context(|| format!("replace temporary Wine {relative} directory"))?;
            } else {
                std::fs::remove_file(&destination)
                    .with_context(|| format!("replace temporary Wine {relative} link"))?;
            }
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&source, &destination)
            .with_context(|| format!("bridge Wine {relative} to the configured game prefix"))?;
    }
    Ok(())
}

fn verify_source_bridges(source_prefix: &Path, tool_prefix: &Path) -> Result<()> {
    let source_user = prefix_user_dir(source_prefix)?;
    let tool_user = prefix_user_dir(tool_prefix)?;
    for relative in ["Documents", "AppData"] {
        let link = tool_user.join(relative);
        let expected = source_user.join(relative);
        let actual = std::fs::read_link(&link)
            .with_context(|| format!("verify the Snap Wine {relative} bridge"))?;
        if actual != expected {
            return Err(anyhow!(
                "The Snap Wine {relative} bridge does not match the configured game prefix"
            ));
        }
    }
    Ok(())
}

fn ensure_game_drive_mappings(
    source_prefix: &Path,
    tool_prefix: &Path,
    game_path: &Path,
) -> Result<()> {
    let canonical_game_path = std::fs::canonicalize(game_path).unwrap_or_else(|_| game_path.into());
    let source_devices = source_prefix.join("dosdevices");
    let tool_devices = tool_prefix.join("dosdevices");
    let Ok(entries) = std::fs::read_dir(&source_devices) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_text = name.to_string_lossy().to_ascii_lowercase();
        if name_text.len() != 2
            || !name_text.ends_with(':')
            || matches!(name_text.as_str(), "c:" | "z:")
        {
            continue;
        }
        let Ok(link_target) = std::fs::read_link(entry.path()) else {
            continue;
        };
        let absolute_target = if link_target.is_absolute() {
            link_target
        } else {
            source_devices.join(link_target)
        };
        let Ok(canonical_target) = std::fs::canonicalize(absolute_target) else {
            continue;
        };
        if !canonical_game_path.starts_with(&canonical_target)
            && !canonical_target.starts_with(&canonical_game_path)
        {
            continue;
        }

        let destination = tool_devices.join(name);
        if destination.symlink_metadata().is_ok() {
            if std::fs::read_link(&destination).is_ok_and(|target| target == canonical_target) {
                continue;
            }
            std::fs::remove_file(&destination).context("replace a Snap Wine game drive mapping")?;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&canonical_target, &destination)
            .context("create a Snap Wine game drive mapping")?;
    }
    Ok(())
}

fn prefix_user_dir(prefix: &Path) -> Result<PathBuf> {
    let users = prefix.join("drive_c/users");
    let steamuser = users.join("steamuser");
    if steamuser.is_dir() {
        return Ok(steamuser);
    }
    let user = std::fs::read_dir(&users)
        .with_context(|| format!("read Wine users in {}", users.display()))?
        .filter_map(|entry| entry.ok())
        .find(|entry| {
            entry.file_name() != "Public" && entry.file_type().is_ok_and(|kind| kind.is_dir())
        })
        .map(|entry| entry.path());
    user.or_else(|| users.join("Public").is_dir().then(|| users.join("Public")))
        .ok_or_else(|| anyhow!("No Wine user directory exists in {}", users.display()))
}

fn configure_prefix(game: &Game, wine_config: &WineConfig, cancel: &AtomicBool) -> Result<()> {
    let (wine_binary, wine_platform, wine_runtime) = snap_runtime(wine_config)?;
    for (name, action) in [
        ("mshtml", "configure Wine DLL overrides"),
        ("winemenubuilder.exe", "disable Wine menu integration"),
    ] {
        let mut command = setup_command(wine_config, &wine_binary, &wine_platform, &wine_runtime);
        command.args([
            "reg",
            "add",
            r"HKCU\Software\Wine\DllOverrides",
            "/v",
            name,
            "/t",
            "REG_SZ",
            "/d",
            "",
            "/f",
        ]);
        run_required(&mut command, Some(cancel), action)?;
    }

    if let Some((registry_key, wine_path)) =
        game::missing_bethesda_reg_key(game, &wine_config.prefix)
    {
        dlog!("deployd: adding registry key {registry_key} → {wine_path}");
        let mut command = setup_command(wine_config, &wine_binary, &wine_platform, &wine_runtime);
        command.args([
            "reg",
            "add",
            &registry_key,
            "/v",
            "Installed Path",
            "/t",
            "REG_SZ",
            "/d",
            &wine_path,
            "/f",
        ]);
        run_required(
            &mut command,
            Some(cancel),
            "register the Bethesda game path",
        )?;
    }
    Ok(())
}

async fn prepare_mono(
    wine_config: &WineConfig,
    cancel: Arc<AtomicBool>,
    on_progress: Arc<dyn Fn(ToolSetupStage) + Send + Sync>,
) -> std::result::Result<(), ToolPrepareError> {
    if wine_config.prefix.join(MONO_MARKER).is_file() {
        return Ok(());
    }
    on_progress(ToolSetupStage::CheckingMono);
    let config = wine_config.clone();
    let version_cancel = cancel.clone();
    let wine_version =
        tokio::task::spawn_blocking(move || detect_wine_version(&config, version_cancel.as_ref()))
            .await
            .map_err(|error| {
                ToolPrepareError::Mono(format!("Wine version check stopped: {error}"))
            })?
            .map_err(|error| mono_error(error, cancel.as_ref()))?;
    mono_version_for_wine(&wine_version)
        .map_err(|error| ToolPrepareError::Mono(error.to_string()))?;

    let config = wine_config.clone();
    let existing_mono = tokio::task::spawn_blocking(move || record_verified_mono(&config))
        .await
        .map_err(|error| {
            ToolPrepareError::Mono(format!("Wine Mono verification stopped: {error}"))
        })?
        .map_err(|error| mono_error(error, cancel.as_ref()))?;
    if existing_mono {
        return Ok(());
    }

    let package = ensure_mono_package(cancel.clone(), on_progress.clone())
        .await
        .map_err(|error| mono_error(error, cancel.as_ref()))?;
    on_progress(ToolSetupStage::InstallingMono);
    let config = wine_config.clone();
    let install_cancel = cancel.clone();
    tokio::task::spawn_blocking(move || install_mono(&config, &package, install_cancel.as_ref()))
        .await
        .map_err(|error| {
            ToolPrepareError::Mono(format!("Wine Mono installation stopped: {error}"))
        })?
        .map_err(|error| mono_error(error, cancel.as_ref()))?;
    Ok(())
}

fn mono_error(error: anyhow::Error, cancel: &AtomicBool) -> ToolPrepareError {
    if cancel.load(std::sync::atomic::Ordering::SeqCst) {
        ToolPrepareError::Cancelled
    } else {
        ToolPrepareError::Mono(error.to_string())
    }
}

fn detect_wine_version(wine_config: &WineConfig, cancel: &AtomicBool) -> Result<String> {
    let (wine_binary, wine_platform, wine_runtime) = snap_runtime(wine_config)?;
    let mut command = setup_command(wine_config, &wine_binary, &wine_platform, &wine_runtime);
    command.arg("--version");
    let output = run_required(&mut command, Some(cancel), "detect the Snap Wine version")?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn mono_version_for_wine(wine_version: &str) -> Result<&'static str> {
    let version = wine_version
        .strip_prefix("wine-")
        .unwrap_or(wine_version)
        .split(['.', '-'])
        .next()
        .unwrap_or("");
    match version.parse::<u32>() {
        Ok(11) => Ok(MONO_VERSION),
        Ok(major) => Err(anyhow!(
            "Wine {major} is not supported by Deployd's verified Wine Mono mapping"
        )),
        Err(_) => Err(anyhow!(
            "Could not understand Wine version '{wine_version}'"
        )),
    }
}

async fn ensure_mono_package(
    cancel: Arc<AtomicBool>,
    on_progress: Arc<dyn Fn(ToolSetupStage) + Send + Sync>,
) -> Result<PathBuf> {
    let cache_dir = paths::snap_wine_mono_cache_dir()?;
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .context("create the Wine Mono download cache")?;
    let cached = cache_dir.join(MONO_FILE_NAME);
    if cached.is_file() {
        on_progress(ToolSetupStage::VerifyingMono);
        match cached_package_action(
            verify_mono_package(&cached, cancel.as_ref()).await,
            runtime::is_cancelled(Some(cancel.as_ref())),
        )? {
            CachedPackageAction::Reuse => return Ok(cached),
            CachedPackageAction::Replace => {}
        }
        tokio::fs::remove_file(&cached)
            .await
            .context("remove a corrupt Wine Mono cache entry")?;
    }

    on_progress(ToolSetupStage::DownloadingMono);
    let temporary = cache_dir.join(format!(".{MONO_FILE_NAME}.{}.part", uuid::Uuid::new_v4()));
    let download_result = download_mono_package(&temporary, cancel.as_ref()).await;
    if let Err(error) = download_result {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }
    on_progress(ToolSetupStage::VerifyingMono);
    if let Err(error) = verify_mono_package(&temporary, cancel.as_ref()).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }
    if cached.exists() {
        let action = cached_package_action(
            verify_mono_package(&cached, cancel.as_ref()).await,
            runtime::is_cancelled(Some(cancel.as_ref())),
        );
        let action = match action {
            Ok(action) => action,
            Err(error) => {
                let _ = tokio::fs::remove_file(&temporary).await;
                return Err(error);
            }
        };
        match action {
            CachedPackageAction::Reuse => {
                let _ = tokio::fs::remove_file(&temporary).await;
                return Ok(cached);
            }
            CachedPackageAction::Replace => {}
        }
        tokio::fs::remove_file(&cached).await?;
    }
    tokio::fs::rename(&temporary, &cached)
        .await
        .context("publish the verified Wine Mono cache entry")?;
    Ok(cached)
}

fn cached_package_action(verification: Result<()>, cancelled: bool) -> Result<CachedPackageAction> {
    match verification {
        Ok(()) => Ok(CachedPackageAction::Reuse),
        Err(error) if cancelled => Err(error),
        Err(_) => Ok(CachedPackageAction::Replace),
    }
}

async fn download_mono_package(destination: &Path, cancel: &AtomicBool) -> Result<()> {
    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(30))
        .build()
        .context("create the Wine Mono download client")?;
    let request = client.get(MONO_URL).send();
    tokio::pin!(request);
    let response = loop {
        tokio::select! {
            response = &mut request => break response.context("download Wine Mono")?,
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                runtime::ensure_not_cancelled(Some(cancel))?;
            }
        }
    }
    .error_for_status()
    .context("Wine Mono download failed")?;
    if let Some(length) = response.content_length() {
        validate_mono_size(length, false)?;
    }

    let mut file = tokio::fs::File::create(destination)
        .await
        .context("create the temporary Wine Mono download")?;
    let mut received = 0_u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        runtime::ensure_not_cancelled(Some(cancel))?;
        let chunk = chunk.context("read the Wine Mono download")?;
        received = received
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| anyhow!("Wine Mono download size overflow"))?;
        validate_mono_size(received, false)?;
        file.write_all(&chunk)
            .await
            .context("write the Wine Mono download")?;
    }
    file.flush().await.context("flush the Wine Mono download")?;
    validate_mono_size(received, true)?;
    Ok(())
}

async fn verify_mono_package(path: &Path, cancel: &AtomicBool) -> Result<()> {
    let metadata = tokio::fs::metadata(path)
        .await
        .context("inspect the Wine Mono package")?;
    validate_mono_size(metadata.len(), true)?;
    let mut file = tokio::fs::File::open(path)
        .await
        .context("open the Wine Mono package for verification")?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        runtime::ensure_not_cancelled(Some(cancel))?;
        let read = file
            .read(&mut buffer)
            .await
            .context("verify the Wine Mono package")?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let actual = format!("{:x}", digest.finalize());
    if !mono_checksum_matches(&actual) {
        return Err(anyhow!("Wine Mono package checksum verification failed"));
    }
    Ok(())
}

fn validate_mono_size(size: u64, complete: bool) -> Result<()> {
    if size > MONO_DOWNLOAD_LIMIT {
        return Err(anyhow!("Wine Mono download exceeds the 90 MB safety limit"));
    }
    if complete && size != MONO_SIZE {
        return Err(anyhow!(
            "Wine Mono package has an unexpected size ({size} bytes; expected {MONO_SIZE})"
        ));
    }
    Ok(())
}

fn mono_checksum_matches(checksum: &str) -> bool {
    checksum.eq_ignore_ascii_case(MONO_SHA256)
}

fn install_mono(wine_config: &WineConfig, package: &Path, cancel: &AtomicBool) -> Result<()> {
    let mut install = mono_install_command(wine_config, package)?;
    run_required(&mut install, Some(cancel), "install Wine Mono")?;

    if !record_verified_mono(wine_config)? {
        return Err(anyhow!(
            "Wine Mono installed without reporting the expected product and version"
        ));
    }
    Ok(())
}

fn mono_install_command(wine_config: &WineConfig, package: &Path) -> Result<Command> {
    let (wine_binary, wine_platform, wine_runtime) = snap_runtime(wine_config)?;
    let mut command = setup_command(wine_config, &wine_binary, &wine_platform, &wine_runtime);
    command.args(["msiexec", "/i"]);
    command.arg(unix_path_to_wine_path(package)?);
    command.arg("/qn");
    Ok(command)
}

fn prefix_init_command(wine_config: &WineConfig) -> Result<Command> {
    let (wine_binary, wine_platform, wine_runtime) = snap_runtime(wine_config)?;
    let mut command = setup_command(wine_config, &wine_binary, &wine_platform, &wine_runtime);
    command.env("WINEDLLOVERRIDES", PREFIX_INIT_DLL_OVERRIDES);
    command.args(["wineboot", "--init"]);
    Ok(command)
}

fn record_verified_mono(wine_config: &WineConfig) -> Result<bool> {
    let registry_path = wine_config.prefix.join("system.reg");
    let registry = match std::fs::read_to_string(&registry_path) {
        Ok(registry) => registry,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("read the Wine registry for Mono verification"),
    };
    if !mono_registry_has_expected_product(&registry) {
        return Ok(false);
    }
    std::fs::write(wine_config.prefix.join(MONO_MARKER), MONO_VERSION)
        .context("record verified Wine Mono setup")?;
    Ok(true)
}

fn mono_registry_has_expected_product(registry: &str) -> bool {
    let mut has_product = false;
    let mut has_version = false;
    for line in registry.lines() {
        if line.starts_with('[') {
            if has_product && has_version {
                return true;
            }
            has_product = false;
            has_version = false;
        }
        let line = line.to_ascii_lowercase();
        if line.contains("displayname") && line.contains("wine mono runtime") {
            has_product = true;
        }
        if line.contains("displayversion") && line.contains(MONO_VERSION) {
            has_version = true;
        }
    }
    has_product && has_version
}

fn unix_path_to_wine_path(path: &Path) -> Result<String> {
    if !path.is_absolute() {
        return Err(anyhow!("Wine Mono cache path is not absolute"));
    }
    Ok(format!("Z:{}", path.to_string_lossy().replace('/', "\\")))
}

fn snap_runtime(wine_config: &WineConfig) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let game::WineLauncher::SnapWine {
        wine_bin,
        wine_platform,
        wine_runtime,
    } = &wine_config.launcher
    else {
        return Err(anyhow!("Snap Wine setup requires the Snap Wine launcher"));
    };
    Ok((
        super::resolve_wine64(wine_bin),
        wine_platform.clone(),
        wine_runtime.clone(),
    ))
}

fn setup_command(
    wine_config: &WineConfig,
    launcher_binary: &Path,
    wine_platform: &Path,
    wine_runtime: &Path,
) -> Command {
    let compat_data = super::strip_pfx_suffix(&wine_config.prefix);
    let mut command = Command::new(launcher_binary);
    command
        .env_remove("LD_PRELOAD")
        .env("WINEPREFIX", &wine_config.prefix)
        .env("WINEDEBUG", "-all")
        .env("STEAM_COMPAT_DATA_PATH", compat_data)
        .env("WINEDLLOVERRIDES", super::WINE_SILENT_DLL_OVERRIDES)
        .env("LD_LIBRARY_PATH", library_path(wine_platform, wine_runtime));
    let drivers_path = dri_drivers_path(wine_runtime);
    command
        .env("LIBGL_DRIVERS_PATH", &drivers_path)
        .env("LIBVA_DRIVERS_PATH", &drivers_path);
    if let Some(ids_path) = amdgpu_ids_path(wine_runtime) {
        command.env("LIBDRM_AMDGPU_IDS", ids_path);
    }
    command
}

fn run_required(
    command: &mut Command,
    cancel: Option<&AtomicBool>,
    action: &str,
) -> Result<Output> {
    let output = runtime::run_output_cancellable(command, cancel)
        .with_context(|| format!("Could not {action}"))?;
    if output.status.success() {
        return Ok(output);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        Err(anyhow!(
            "Could not {action}: Wine exited with {}",
            output.status
        ))
    } else {
        Err(anyhow!(
            "Could not {action}: Wine exited with {}: {stderr}",
            output.status
        ))
    }
}

pub(super) fn library_path(wine_platform: &Path, wine_runtime: &Path) -> String {
    format!(
        "{p}/lib:{p}/lib64:{r}/lib:{r}/$LIB:{r}/usr/lib:{r}/usr/$LIB:{r}/usr/$LIB/dri:{r}/usr/$LIB/pulseaudio:{r}/usr/$LIB/samba",
        p = wine_platform.display(),
        r = wine_runtime.display(),
    )
}

fn dri_drivers_path(wine_runtime: &Path) -> String {
    format!(
        "{r}/usr/lib/x86_64-linux-gnu/dri:{r}/usr/lib/i386-linux-gnu/dri",
        r = wine_runtime.display(),
    )
}

fn amdgpu_ids_path(wine_runtime: &Path) -> Option<String> {
    let path = wine_runtime.join("usr/share/libdrm/amdgpu.ids");
    path.exists().then(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;
    use crate::core::game::WineLauncher;
    use crate::models::game::GameEngine;

    fn environment(command: &Command, key: &str) -> Option<String> {
        command.get_envs().find_map(|(name, value)| {
            (name == OsStr::new(key))
                .then(|| value.map(|value| value.to_string_lossy().into_owned()))
                .flatten()
        })
    }

    fn test_config(prefix: PathBuf) -> WineConfig {
        WineConfig {
            prefix,
            launcher: WineLauncher::SnapWine {
                wine_bin: PathBuf::from("/snap/wine-platform/current/bin/wine64"),
                wine_platform: PathBuf::from("/snap/wine-platform/current"),
                wine_runtime: PathBuf::from("/snap/wine-runtime/current"),
            },
        }
    }

    // @variants: snap
    #[test]
    fn snap_plan_uses_only_owned_prefix_and_content_runtime_paths() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let executable = temp.path().join("tools/xEdit.exe");
        std::fs::create_dir_all(executable.parent().unwrap_or(temp.path()))?;
        std::fs::write(&executable, b"")?;
        let game = Game {
            id: "skyrim-se".to_string(),
            title: "Skyrim Special Edition".to_string(),
            path: temp.path().join("game"),
            data_subdir: "Data".to_string(),
            engine: GameEngine::Bethesda,
            wine_prefix: Some(temp.path().join("heroic-prefix/pfx")),
        };
        let tool = Tool {
            id: "xedit".to_string(),
            game_id: game.id.clone(),
            name: "xEdit".to_string(),
            exe_path: executable.to_string_lossy().into_owned(),
            icon_name: String::new(),
            custom_args: String::new(),
            sort_order: 0,
            working_dir: String::new(),
        };
        let owned_prefix = temp.path().join("snap-common/wine-prefixes/skyrim-se");
        let config = test_config(owned_prefix.clone());
        let WineLauncher::SnapWine {
            wine_bin,
            wine_platform,
            wine_runtime,
        } = &config.launcher
        else {
            unreachable!()
        };

        let command = build_command(wine_bin, wine_platform, wine_runtime, &tool, &game, &config);

        assert_eq!(
            environment(&command, "WINEPREFIX"),
            Some(owned_prefix.display().to_string())
        );
        assert!(!format!("{command:?}").contains("heroic-prefix"));
        assert!(environment(&command, "LD_LIBRARY_PATH").is_some());
        Ok(())
    }

    // @variants: snap
    #[test]
    fn source_prefix_bridges_preserve_bethesda_and_eclipse_user_state() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        for prefix in [&source, &target] {
            std::fs::create_dir_all(prefix.join("drive_c/users/steamuser/Documents"))?;
            std::fs::create_dir_all(prefix.join("drive_c/users/steamuser/AppData"))?;
        }
        std::fs::create_dir_all(
            source.join("drive_c/users/steamuser/Documents/My Games/Skyrim Special Edition"),
        )?;
        std::fs::write(
            source.join(
                "drive_c/users/steamuser/Documents/My Games/Skyrim Special Edition/Skyrim.ini",
            ),
            b"bethesda",
        )?;
        std::fs::create_dir_all(
            source.join("drive_c/users/steamuser/Documents/BioWare/Dragon Age"),
        )?;
        std::fs::write(
            source.join("drive_c/users/steamuser/Documents/BioWare/Dragon Age/settings.ini"),
            b"eclipse",
        )?;

        create_source_bridges(&source, &target)?;
        verify_source_bridges(&source, &target)?;

        assert_eq!(
            std::fs::read_link(target.join("drive_c/users/steamuser/Documents"))?,
            source.join("drive_c/users/steamuser/Documents")
        );
        assert_eq!(
            std::fs::read_link(target.join("drive_c/users/steamuser/AppData"))?,
            source.join("drive_c/users/steamuser/AppData")
        );
        assert_eq!(
            std::fs::read(target.join(
                "drive_c/users/steamuser/Documents/My Games/Skyrim Special Edition/Skyrim.ini"
            ))?,
            b"bethesda"
        );
        assert_eq!(
            std::fs::read(
                target.join("drive_c/users/steamuser/Documents/BioWare/Dragon Age/settings.ini")
            )?,
            b"eclipse"
        );
        Ok(())
    }

    // @variants: snap
    #[test]
    fn game_drive_mapping_excludes_source_runtime_links() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        let game_volume = temp.path().join("games");
        let hidden_runtime = temp.path().join(".config/heroic/runtime");
        std::fs::create_dir_all(source.join("dosdevices"))?;
        std::fs::create_dir_all(target.join("dosdevices"))?;
        std::fs::create_dir_all(game_volume.join("Skyrim"))?;
        std::fs::create_dir_all(&hidden_runtime)?;
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&game_volume, source.join("dosdevices/g:"))?;
            std::os::unix::fs::symlink(&hidden_runtime, source.join("dosdevices/r:"))?;
        }

        ensure_game_drive_mappings(&source, &target, &game_volume.join("Skyrim"))?;

        assert_eq!(
            std::fs::read_link(target.join("dosdevices/g:"))?,
            game_volume
        );
        assert!(target.join("dosdevices/r:").symlink_metadata().is_err());
        Ok(())
    }

    // @variants: snap
    #[test]
    fn wine_11_maps_to_verified_mono_version() -> Result<()> {
        assert_eq!(mono_version_for_wine("wine-11.0")?, MONO_VERSION);
        assert!(mono_version_for_wine("wine-12.0").is_err());
        assert!(mono_version_for_wine("unknown").is_err());
        Ok(())
    }

    // @variants: snap
    #[test]
    fn silent_msi_command_uses_qn() -> Result<()> {
        let package = Path::new("/snap-common/wine-runtime/wine-mono.msi");
        assert_eq!(
            unix_path_to_wine_path(package)?,
            r"Z:\snap-common\wine-runtime\wine-mono.msi"
        );
        let config = test_config(PathBuf::from("/snap-common/prefix"));
        let command = mono_install_command(&config, package)?;
        assert_eq!(
            command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            [
                "msiexec",
                "/i",
                r"Z:\snap-common\wine-runtime\wine-mono.msi",
                "/qn"
            ]
        );
        Ok(())
    }

    // @variants: snap
    #[test]
    fn mono_product_verification_requires_name_and_version() {
        assert!(mono_registry_has_expected_product(
            "[Software\\Mono] 1\n\
             \"DisplayName\"=\"Wine Mono Runtime\"\n\
             \"DisplayVersion\"=\"10.4.1\"\n"
        ));
        assert!(!mono_registry_has_expected_product(
            "[Software\\Mono] 1\n\
             \"DisplayName\"=\"Wine Mono Runtime\"\n\
             \"DisplayVersion\"=\"9.4.0\"\n"
        ));
        assert!(!mono_registry_has_expected_product(
            "[Software\\Mono] 1\n\
             \"DisplayName\"=\"Wine Mono Runtime\"\n\n\
             [Software\\Other] 2\n\
             \"DisplayVersion\"=\"10.4.1\"\n"
        ));
    }

    // @variants: snap
    #[test]
    fn existing_verified_mono_is_recorded_without_reinstalling() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config = test_config(temp.path().to_path_buf());
        std::fs::write(
            temp.path().join("system.reg"),
            "[Software\\Mono] 1\n\
             \"DisplayName\"=\"Wine Mono Runtime\"\n\
             \"DisplayVersion\"=\"10.4.1\"\n",
        )?;

        assert!(record_verified_mono(&config)?);
        assert_eq!(
            std::fs::read_to_string(temp.path().join(MONO_MARKER))?,
            MONO_VERSION
        );
        Ok(())
    }

    // @variants: snap
    #[test]
    fn prefix_initialization_suppresses_the_wine_mono_prompt() -> Result<()> {
        let config = test_config(PathBuf::from("/snap-common/prefix"));
        let command = prefix_init_command(&config)?;
        assert_eq!(
            environment(&command, "WINEDLLOVERRIDES"),
            Some(PREFIX_INIT_DLL_OVERRIDES.to_string())
        );
        assert_eq!(
            command
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            ["wineboot", "--init"]
        );
        Ok(())
    }

    // @variants: snap
    #[test]
    fn setup_removes_host_preload() {
        let config = test_config(PathBuf::from("/snap-common/prefix"));
        let (wine, platform, runtime) = snap_runtime(&config).expect("Snap config");
        let command = setup_command(&config, &wine, &platform, &runtime);
        assert!(
            command
                .get_envs()
                .any(|(name, value)| { name == OsStr::new("LD_PRELOAD") && value.is_none() })
        );
    }

    // @variants: snap
    #[test]
    fn prefix_readiness_requires_transaction_marker_and_wine_files() -> Result<()> {
        let temp = tempfile::tempdir()?;
        assert!(!prefix_is_initialized(temp.path()));
        std::fs::write(temp.path().join(PREFIX_MARKER), b"1")?;
        std::fs::write(temp.path().join("system.reg"), b"")?;
        std::fs::create_dir_all(temp.path().join("dosdevices"))?;
        #[cfg(unix)]
        std::os::unix::fs::symlink("../drive_c", temp.path().join("dosdevices/c:"))?;
        assert!(prefix_is_initialized(temp.path()));
        Ok(())
    }

    // @variants: snap
    #[test]
    fn setup_console_is_needed_only_until_the_prefix_is_ready() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config = test_config(temp.path().to_path_buf());
        assert!(initial_setup_required(&config));

        std::fs::write(temp.path().join(PREFIX_MARKER), b"1")?;
        std::fs::write(temp.path().join("system.reg"), b"")?;
        std::fs::create_dir_all(temp.path().join("dosdevices"))?;
        #[cfg(unix)]
        std::os::unix::fs::symlink("../drive_c", temp.path().join("dosdevices/c:"))?;
        assert!(!initial_setup_required(&config));
        Ok(())
    }

    // @variants: snap
    #[test]
    fn mono_download_enforces_limit_and_exact_final_size() -> Result<()> {
        validate_mono_size(MONO_SIZE, true)?;
        assert!(validate_mono_size(MONO_SIZE - 1, true).is_err());
        assert!(validate_mono_size(MONO_DOWNLOAD_LIMIT + 1, false).is_err());
        Ok(())
    }

    // @variants: snap
    #[test]
    fn mono_checksum_rejects_unverified_content() {
        assert!(mono_checksum_matches(MONO_SHA256));
        assert!(!mono_checksum_matches(
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
        ));
    }

    // @variants: snap
    #[test]
    fn mono_cache_reuses_only_verified_packages() -> Result<()> {
        assert_eq!(
            cached_package_action(Ok(()), false)?,
            CachedPackageAction::Reuse
        );
        assert_eq!(
            cached_package_action(Err(anyhow!("checksum mismatch")), false)?,
            CachedPackageAction::Replace
        );
        assert!(cached_package_action(Err(anyhow!("cancelled")), true).is_err());
        Ok(())
    }

    // @variants: snap
    #[test]
    fn native_tool_continuation_skips_mono_but_retry_does_not() {
        assert!(!should_prepare_mono(true));
        assert!(should_prepare_mono(false));
    }

    // @variants: snap
    #[test]
    fn cancellation_interrupts_mono_setup() {
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            mono_error(anyhow!("interrupted"), &cancelled),
            ToolPrepareError::Cancelled
        ));
    }
}
