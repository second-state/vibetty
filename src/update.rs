//! `vibetty update` —— 自更新子命令。
//!
//! 行为与 `install.sh` 一致:从 GitHub Releases 下载对应平台的预编译二进制,
//! 装成 `vibetty-<版本>`(Windows 加 .exe),再把 `vibetty` 软链/副本切过去。
//! 升级不覆盖正在运行的二进制;同版本 + 软链完好时提示并退出。
//!
//! 交互提示(旧版本清理 / PATH rc)只在有终端时出现;`VIBETTY_NONINTERACTIVE=1`
//! 或无 tty 时全部走安全默认(保留旧版本、不动 rc)。

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "second-state/vibetty";

/// 检测 (资产名, 安装文件名)。与 install.sh / release.yml 的命名一致。
fn detect() -> Result<(String, String), String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    match (os, arch) {
        ("linux", "x86_64") => Ok(("vibetty-linux-x64".into(), "vibetty".into())),
        ("macos", "aarch64") => Ok(("vibetty-macos-arm64".into(), "vibetty".into())),
        ("windows", "x86_64") => Ok(("vibetty-windows-x64.exe".into(), "vibetty.exe".into())),
        (os, arch) => Err(format!(
            "no prebuilt binary for {os}/{arch} — check https://github.com/{REPO}/releases"
        )),
    }
}

/// 安装目录:当前二进制所在目录(自然覆盖 cargo install / install.sh 的位置),
/// 拿不到则回退 `~/.cargo/bin`。
fn install_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .filter(|d| d.exists())
        .unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(".cargo").join("bin"))
                .unwrap_or_else(|| PathBuf::from("."))
        })
}

fn is_interactive() -> bool {
    std::io::stdin().is_terminal() && std::env::var("VIBETTY_NONINTERACTIVE").as_deref() != Ok("1")
}

/// 下载(reqwest async + rustls:复用依赖树里已有的 TLS 栈,不依赖外部 curl)。
async fn download(url: &str, dest: &Path) -> Result<(), String> {
    let resp = reqwest::get(url)
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("download failed: {e}"))?;
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("download read failed: {e}"))?;
    std::fs::write(dest, &bytes).map_err(|e| format!("write {}: {e}", dest.display()))
}

/// 入口:`vibetty update [TAG]`。TAG 缺省 = latest(不含 prerelease)。
pub async fn run(tag: Option<String>) -> Result<(), String> {
    let (asset, name) = detect()?;
    let tag = tag.unwrap_or_else(|| "latest".to_string());
    let install_dir = install_dir();

    let url = if tag == "latest" {
        format!("https://github.com/{REPO}/releases/latest/download/{asset}")
    } else {
        format!("https://github.com/{REPO}/releases/download/{tag}/{asset}")
    };

    std::fs::create_dir_all(&install_dir).map_err(|e| e.to_string())?;
    let staging = install_dir.join(format!(".vibetty.update.{}", std::process::id()));

    println!("Downloading {url}");
    download(&url, &staging).await?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755));
    }

    // 版本号直接问二进制(输出如 "vibetty 0.4.2")。
    let out = Command::new(&staging)
        .arg("--version")
        .output()
        .map_err(|e| format!("cannot run the downloaded binary: {e}"))?;
    let binver = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_string();
    if binver.is_empty() {
        let _ = std::fs::remove_file(&staging);
        return Err("cannot read version from the downloaded binary".to_string());
    }

    #[cfg(windows)]
    {
        let target = install_dir.join(&name); // vibetty.exe
        // 同版本已装 → up to date。
        if target.exists()
            && let Ok(out) = Command::new(&target).arg("--version").output()
            && String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .nth(1)
                == Some(binver.as_str())
        {
            let _ = std::fs::remove_file(&staging);
            println!(
                "vibetty {binver} is already installed at {} — up to date",
                target.display()
            );
            return Ok(());
        }
        // 先把旧 vibetty.exe 改名成它的版本号(Windows 允许 rename 运行中的 exe,
        // 不允许覆盖),再直接把新 exe 改名到位 —— 顺序固定,不做失败重试。
        if target.exists() {
            let oldver = Command::new(&target)
                .arg("--version")
                .output()
                .ok()
                .and_then(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .split_whitespace()
                        .nth(1)
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| "old".to_string());
            let old_path = install_dir.join(format!("vibetty-{oldver}.exe"));
            let _ = std::fs::remove_file(&old_path);
            std::fs::rename(&target, &old_path).map_err(|e| format!("cannot move old exe: {e}"))?;
        }
        std::fs::rename(&staging, &target).map_err(|e| e.to_string())?;
        println!("Installed: {} ({binver})", target.display());
    }

    #[cfg(unix)]
    {
        let versioned = install_dir.join(format!("vibetty-{binver}"));
        let link = install_dir.join("vibetty");
        // 同版本已装且链接完好 → up to date。
        if versioned.exists() && std::fs::read_link(&link).is_ok_and(|l| l == versioned) {
            let _ = std::fs::remove_file(&staging);
            println!(
                "vibetty {binver} is already installed at {} — up to date",
                versioned.display()
            );
            return Ok(());
        }
        std::fs::rename(&staging, &versioned).map_err(|e| e.to_string())?;
        std::os::unix::fs::symlink(&versioned, &link).map_err(|e| e.to_string())?;
        println!(
            "Installed: {} -> {} ({binver})",
            versioned.display(),
            link.display()
        );
    }

    // 当前平台新装的文件路径(供旧版本清理排除用)。
    #[cfg(unix)]
    let installed_file = install_dir.join(format!("vibetty-{binver}"));
    #[cfg(windows)]
    let installed_file = install_dir.join(&name);

    // --- 旧版本清理:列出 vibetty-<数字>* 历史版本,交互时询问是否删除(默认 N)。 ---
    let prefix = format!("{}-", name.trim_end_matches(".exe"));
    let olds: Vec<PathBuf> = std::fs::read_dir(&install_dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                    n.starts_with(&prefix)
                        && n[prefix.len()..].starts_with(|c: char| c.is_ascii_digit())
                })
        })
        .filter(|p| *p != installed_file)
        .collect();
    if !olds.is_empty() {
        println!("Found old version(s):");
        for p in &olds {
            println!("  {}", p.display());
        }
        if is_interactive() {
            use dialoguer::Confirm;
            let delete = Confirm::new()
                .with_prompt("Delete old version(s)?")
                .default(false)
                .interact()
                .unwrap_or(false);
            if delete {
                for p in &olds {
                    let _ = std::fs::remove_file(p);
                    println!("Removed: {}", p.display());
                }
            } else {
                println!("kept — delete manually if no longer needed");
            }
        } else {
            println!("kept — delete manually if no longer needed");
        }
    }

    // --- PATH 检查:不在 PATH 就提示(仅交互时询问写入 rc;这里简化为提示)。 ---
    let dir_str = install_dir.display().to_string();
    let in_path = std::env::var("PATH")
        .map(|p| p.split(':').chain(p.split(';')).any(|s| s == dir_str))
        .unwrap_or(false);
    if !in_path {
        println!("note: {dir_str} is not on your PATH — add it to your shell rc if needed");
    }

    Ok(())
}
