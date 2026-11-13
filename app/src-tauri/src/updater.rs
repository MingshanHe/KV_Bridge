//! Updating from the "Latest build" GitHub release.
//!
//! Each build publishes `latest.json` with the version and the SHA-256 of both
//! installers. The app downloads the installer for its platform, checks the hash,
//! installs it and starts again.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tauri::AppHandle;

const LATEST: &str = "https://github.com/MingshanHe/KV_Bridge/releases/download/latest/latest.json";

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Latest {
    pub version: String,
    // Each platform reads only its own entry.
    #[allow(dead_code)]
    pub windows: Asset,
    #[allow(dead_code)]
    pub macos: Asset,
}

fn parse(v: &str) -> Vec<u64> {
    v.trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Is `a` newer than `b`?
pub fn newer(a: &str, b: &str) -> bool {
    parse(a) > parse(b)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(60))
        .build()
}

/// The latest release, if it is newer than `current`.
pub fn check(current: &str) -> Result<Option<Latest>, String> {
    let body = agent()
        .get(LATEST)
        .call()
        .map_err(|e| format!("检查更新失败：{e}"))?
        .into_string()
        .map_err(|e| e.to_string())?;
    let l: Latest = serde_json::from_str(&body).map_err(|e| format!("更新信息格式不对：{e}"))?;
    Ok(newer(&l.version, current).then_some(l))
}

/// Download `asset` to `path`, checking its SHA-256.
fn download(asset: &Asset, path: &Path) -> Result<(), String> {
    let resp = agent().get(&asset.url).call().map_err(|e| format!("下载失败：{e}"))?;
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("下载中断：{e}"))?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
    }
    drop(file);
    let got = kvm_net::to_hex(&hash.finalize());
    if !got.eq_ignore_ascii_case(asset.sha256.trim()) {
        let _ = std::fs::remove_file(path);
        return Err("下载的文件校验不通过，已删除。请稍后再试".into());
    }
    Ok(())
}

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(name)
}

/// Download, install and restart. Returns only on failure.
pub fn install(app: &AppHandle, l: &Latest) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let setup = temp("KV-Bridge-Setup.exe");
        download(&l.windows, &setup)?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // Wait for this app to quit, install silently, start the new version.
        let script = format!(
            "@echo off\r\nping -n 3 127.0.0.1 >nul\r\n\"{}\" /S\r\nstart \"\" \"{}\"\r\n",
            setup.display(),
            exe.display()
        );
        let bat = temp("kv-bridge-update.cmd");
        std::fs::write(&bat, script).map_err(|e| e.to_string())?;
        std::process::Command::new("cmd")
            .arg("/C")
            .arg(&bat)
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn()
            .map_err(|e| format!("无法启动安装程序：{e}"))?;
        app.exit(0);
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let bundle = exe
            .ancestors()
            .nth(3)
            .filter(|p| p.extension().is_some_and(|e| e == "app"))
            .ok_or("找不到 KV Bridge.app 的位置")?
            .to_path_buf();
        let parent = bundle.parent().ok_or("找不到 KV Bridge.app 的位置")?;
        if bundle.starts_with("/Volumes") {
            return Err("请先把 KV Bridge 拖到「应用程序」文件夹，再更新".into());
        }
        // Can we replace the app where it is? (Not on accounts without admin rights.)
        let probe = parent.join(".kv-bridge-write-test");
        if std::fs::write(&probe, b"").is_err() {
            return Err(format!("没有权限写入 {}。请手动下载新版本安装", parent.display()));
        }
        let _ = std::fs::remove_file(&probe);

        let dmg = temp("KV-Bridge-update.dmg");
        download(&l.macos, &dmg)?;
        let q = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "'\\''"));
        let (b, d) = (q(&bundle), q(&dmg));
        let script = format!(
            "sleep 1\n\
             MNT=$(mktemp -d)\n\
             hdiutil attach -nobrowse -readonly -mountpoint \"$MNT\" {d} || exit 1\n\
             rm -rf {b}.old\n\
             if mv {b} {b}.old && cp -R \"$MNT/KV Bridge.app\" {b}; then rm -rf {b}.old; else rm -rf {b}; mv {b}.old {b}; fi\n\
             hdiutil detach -quiet \"$MNT\"\n\
             rm -f {d}\n\
             xattr -dr com.apple.quarantine {b} 2>/dev/null\n\
             open {b}\n"
        );
        std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .spawn()
            .map_err(|e| format!("无法开始安装：{e}"))?;
        app.exit(0);
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (app, l, download as fn(&Asset, &Path) -> Result<(), String>, temp as fn(&str) -> PathBuf);
        Err("这个系统不支持自动更新".into())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_order() {
        assert!(super::newer("0.4.12", "0.4.9"));
        assert!(super::newer("0.5.0", "0.4.99"));
        assert!(!super::newer("0.4.9", "0.4.9"));
        assert!(!super::newer("0.3.0", "0.4.1"));
    }
}
