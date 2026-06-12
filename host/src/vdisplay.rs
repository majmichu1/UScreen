use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tracing::{error, info};

pub struct VirtualDisplayManager {
    child: Option<Child>,
    edid_path: PathBuf,
    helper_path: PathBuf,
    card: Option<u32>,
}

impl VirtualDisplayManager {
    pub fn new(helper_path: &Path, edid_path: &Path) -> Self {
        Self {
            child: None,
            edid_path: edid_path.into(),
            helper_path: helper_path.into(),
            card: None,
        }
    }

    pub async fn create(&mut self) -> Result<String> {
        let mut cmd = Command::new("sudo");
        cmd.arg(&self.helper_path)
            .args(["--edid", &self.edid_path.to_string_lossy()])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .stdin(Stdio::null())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .context("Failed to spawn evdi-helper (needs sudo for EVDI device access)")?;

        let stdout = child
            .stdout
            .take()
            .context("Failed to capture evdi-helper stdout")?;
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        let mut card_num = None;
        let timeout = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while let Some(line) = lines.next_line().await.transpose() {
                let line = line?;
                if let Some(card_str) = line.strip_prefix("EVDI_CONNECTED card") {
                    card_num = Some(card_str.parse::<u32>()?);
                    info!(
                        "EVDI virtual display connected on card{}",
                        card_num.unwrap()
                    );
                    break;
                }
            }
            anyhow::Ok(card_num)
        })
        .await;

        match timeout {
            Ok(Ok(Some(card))) => {
                self.card = Some(card);
                self.child = Some(child);
                Ok(format!("DVI-I-{}", card))
            }
            Ok(Ok(None)) => {
                error!("evdi-helper exited without reporting connection");
                anyhow::bail!("evdi-helper failed to connect");
            }
            Ok(Err(e)) => {
                error!("evdi-helper communication error: {}", e);
                anyhow::bail!("evdi-helper communication error: {}", e);
            }
            Err(_) => {
                error!("evdi-helper timed out (10s)");
                child.kill().await?;
                anyhow::bail!("evdi-helper timed out");
            }
        }
    }

    pub fn card(&self) -> Option<u32> {
        self.card
    }

    pub fn display_name(&self) -> Option<String> {
        self.card.map(|c| format!("DVI-I-{}", c))
    }
}
