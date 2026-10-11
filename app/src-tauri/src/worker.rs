//! Pilote du worker vision Python (JSON Lines sur stdin/stdout).
//!
//! Un thread lit stdout : les réponses débloquent la requête en attente portant le même `id`,
//! les événements de progression sont relayés au frontend.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::error::{err, AppError, Result};
use crate::env::Tools;

type Pending = Arc<Mutex<HashMap<String, Sender<Value>>>>;

pub struct Worker {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    pending: Pending,
    next_id: AtomicU64,
    pub root: PathBuf,
}

impl Worker {
    /// Démarre le worker, limité en lecture/écriture au dossier `root` (le projet).
    pub fn spawn(tools: &Tools, root: &Path, on_progress: impl Fn(Value) + Send + 'static) -> Result<Arc<Worker>> {
        if !tools.python.exists() {
            return err(format!("Python du worker vision introuvable : {}", tools.python.display()));
        }
        let mut child = Command::new(&tools.python)
            .arg("-u")
            .arg(&tools.worker)
            .arg("--models-dir").arg(&tools.models)
            .arg("--root").arg(root)
            .env("PYTORCH_ENABLE_MPS_FALLBACK", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| AppError::Msg(format!("Impossible de lancer le worker vision : {e}")))?;
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (ready_tx, ready_rx) = channel::<Value>();

        let pend = pending.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
                let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
                match msg.get("event").and_then(|e| e.as_str()) {
                    Some("progress") => on_progress(msg),
                    Some("ready") => { let _ = ready_tx.send(msg); }
                    _ => {
                        if let Some(id) = msg.get("id").and_then(|i| i.as_str()) {
                            if let Some(tx) = pend.lock().unwrap().remove(id) {
                                let _ = tx.send(msg);
                            }
                        }
                    }
                }
            }
            // Le worker s'est arrêté : on débloque toutes les requêtes en attente.
            for (_, tx) in pend.lock().unwrap().drain() {
                let _ = tx.send(json!({"ok": false, "error": "Le worker vision s'est arrêté."}));
            }
        });
        // stderr du worker : journal de diagnostic (warnings PyTorch, traces d'erreur).
        let log_path = root.join("logs/worker.log");
        std::thread::spawn(move || {
            let mut log = std::fs::OpenOptions::new().create(true).append(true).open(log_path).ok();
            for line in BufReader::new(stderr).lines().map_while(|l| l.ok()) {
                if let Some(f) = log.as_mut() {
                    let _ = writeln!(f, "{line}");
                }
            }
        });

        ready_rx.recv_timeout(Duration::from_secs(60))
            .map_err(|_| AppError::Msg("Le worker vision n'a pas démarré (voir logs/worker.log du projet).".into()))?;
        Ok(Arc::new(Worker {
            stdin: Mutex::new(stdin),
            child: Mutex::new(child),
            pending,
            next_id: AtomicU64::new(1),
            root: root.to_path_buf(),
        }))
    }

    pub fn is_alive(&self) -> bool {
        matches!(self.child.lock().unwrap().try_wait(), Ok(None))
    }

    fn send_line(&self, msg: &Value) -> Result<()> {
        let mut stdin = self.stdin.lock().unwrap();
        writeln!(stdin, "{msg}")?;
        stdin.flush()?;
        Ok(())
    }

    /// Envoie une requête et attend la réponse (bloquant : à appeler hors du thread principal).
    pub fn request(&self, op: &str, params: Value) -> Result<Value> {
        let id = format!("r{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        if let Err(e) = self.send_line(&json!({"id": id, "op": op, "params": params})) {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        let resp = rx.recv().map_err(|_| AppError::Msg("Le worker vision ne répond plus.".into()))?;
        if resp.get("ok").and_then(|o| o.as_bool()) == Some(true) {
            Ok(resp.get("result").cloned().unwrap_or(Value::Null))
        } else {
            err(resp.get("error").and_then(|e| e.as_str()).unwrap_or("Erreur inconnue du worker.").to_string())
        }
    }

    pub fn cancel(&self) -> Result<()> {
        self.send_line(&json!({"id": "cancel", "op": "cancel"}))
    }

    pub fn shutdown(&self) {
        let _ = self.send_line(&json!({"id": "bye", "op": "shutdown"}));
        let mut child = self.child.lock().unwrap();
        for _ in 0..20 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = child.kill();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}
