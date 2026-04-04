use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::oneshot;
use tokio::sync::mpsc;
use std::collections::HashMap;

use super::config::McpServerConfig;

pub struct StdioTransport {
    pub(crate) child: Child,
    pub(crate) next_id: u64,
    pub(crate) request_tx: mpsc::Sender<(Value, oneshot::Sender<Result<Value>>)>,
    pub(crate) notify_tx: mpsc::Sender<Value>,
}

impl StdioTransport {
    pub async fn spawn(cfg: &McpServerConfig) -> Result<Self> {
        let mut command = Command::new(&cfg.command);
        command.args(&cfg.args);
        command.envs(&cfg.env);

        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        // Stderr is inherited so user can see errors
        command.stderr(Stdio::inherit());

        let mut child = command.spawn().context("Failed to spawn MCP server")?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        let (request_tx, request_rx) = mpsc::channel(32);
        let (notify_tx, notify_rx) = mpsc::channel(32);

        // Spawn IO loops
        tokio::spawn(Self::io_loop(stdin, stdout, request_rx, notify_rx));

        Ok(Self {
            child,
            next_id: 1,
            request_tx,
            notify_tx,
        })
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;

        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let (resp_tx, resp_rx) = oneshot::channel();
        self.request_tx.send((req, resp_tx)).await?;

        resp_rx.await?
    }

    pub async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let req = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        self.notify_tx.send(req).await?;
        Ok(())
    }

    pub async fn shutdown(&mut self) {
        let _ = self.child.kill().await;
    }

    async fn io_loop(
        mut stdin: ChildStdin,
        stdout: ChildStdout,
        mut request_rx: mpsc::Receiver<(Value, oneshot::Sender<Result<Value>>)>,
        mut notify_rx: mpsc::Receiver<Value>,
    ) {
        let mut reader = BufReader::new(stdout).lines();
        let mut pending_requests: HashMap<u64, oneshot::Sender<Result<Value>>> = HashMap::new();

        loop {
            tokio::select! {
                // Read from stdout
                line_opt = reader.next_line() => {
                    match line_opt {
                        Ok(Some(line)) => {
                            if let Ok(resp) = serde_json::from_str::<Value>(&line) {
                                if let Some(id) = resp.get("id").and_then(|i| i.as_u64()) {
                                    if let Some(tx) = pending_requests.remove(&id) {
                                        if let Some(err) = resp.get("error") {
                                            let _ = tx.send(Err(anyhow::anyhow!("RPC Error: {}", err)));
                                        } else {
                                            let result = resp.get("result").cloned().unwrap_or(Value::Null);
                                            let _ = tx.send(Ok(result));
                                        }
                                    }
                                }
                                // Ignore notifications and requests from server for now
                            }
                        }
                        Ok(None) | Err(_) => break, // EOF or error
                    }
                }
                
                // Write requests
                req_opt = request_rx.recv() => {
                    match req_opt {
                        Some((req, resp_tx)) => {
                            if let Some(id) = req.get("id").and_then(|i| i.as_u64()) {
                                pending_requests.insert(id, resp_tx);
                            }
                            
                            if let Ok(mut bytes) = serde_json::to_vec(&req) {
                                bytes.push(b'\n');
                                if stdin.write_all(&bytes).await.is_err() {
                                    break;
                                }
                                let _ = stdin.flush().await;
                            }
                        }
                        None => break,
                    }
                }

                // Write notifications
                notif_opt = notify_rx.recv() => {
                    match notif_opt {
                        Some(notif) => {
                            if let Ok(mut bytes) = serde_json::to_vec(&notif) {
                                bytes.push(b'\n');
                                if stdin.write_all(&bytes).await.is_err() {
                                    break;
                                }
                                let _ = stdin.flush().await;
                            }
                        }
                        None => break,
                    }
                }
            }
        }
    }
}
