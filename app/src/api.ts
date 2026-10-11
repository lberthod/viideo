// Contrat typé avec le noyau Rust (src-tauri/src/commands.rs). Le frontend propose, Rust valide.
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface ToolStatus { name: string; path: string; ok: boolean; detail: string }

export interface Probe {
  duration: number; width: number; height: number; display_width: number; display_height: number;
  rotation: number; fps_num: number; fps_den: number; codec: string; has_audio: boolean; size_bytes: number;
}

export interface Point { x: number; y: number; positive: boolean }
export interface Prompt { frame: number; points: Point[]; box: [number, number, number, number] | null }

export interface Composite {
  background: string | null; background_color: string;
  subject_scale: number; subject_x: number; subject_y: number;
  feather: number; brightness: number; contrast: number; saturation: number;
}

export interface ExportRecord { path: string; created_at: string; seconds: number; width: number; height: number; size_bytes: number }

export interface Manifest {
  schema_version: number; app_version: string; created_at: string; updated_at: string;
  source: { path: string; fingerprint: string; probe: Probe };
  clip: { start: number; duration: number };
  proxy: { key: string; dir: string; width: number; height: number; frames: number; fps_num: number; fps_den: number } | null;
  segmentation: {
    model: string; dtype: string; prompts: Prompt[]; status: "none" | "stale" | "ready";
    mask: { key: string; dir: string; frames: number; seconds: number; empty_frames: number } | null;
  };
  composite: Composite;
  exports: ExportRecord[];
}

export interface ProjectView {
  dir: string; manifest: Manifest; source_missing: boolean;
  proxy_dir: string | null; mask_dir: string | null; background_path: string | null;
}

export interface PreviewResult { frame: number; png: string; area_pct: number }

export const api = {
  checkEnvironment: () => invoke<ToolStatus[]>("check_environment"),
  probeVideo: (path: string) => invoke<Probe>("probe_video", { path }),
  createProject: (projectDir: string, source: string) => invoke<ProjectView>("create_project", { projectDir, source }),
  openProject: (projectDir: string) => invoke<ProjectView>("open_project", { projectDir }),
  devAutoload: () => invoke<ProjectView | null>("dev_autoload"),
  devDemo: () => invoke<string | null>("dev_demo"),
  devLog: (msg: string) => invoke<void>("dev_log", { msg }),
  closeProject: () => invoke<void>("close_project"),
  setClip: (start: number, duration: number) => invoke<ProjectView>("set_clip", { start, duration }),
  prepareProxy: () => invoke<ProjectView>("prepare_proxy"),
  setPrompts: (prompts: Prompt[]) => invoke<ProjectView>("set_prompts", { prompts }),
  previewMask: (frame: number) => invoke<PreviewResult>("preview_mask", { frame }),
  propagateMask: () => invoke<ProjectView>("propagate_mask"),
  cancelJob: () => invoke<void>("cancel_job"),
  unloadModels: () => invoke<void>("unload_models"),
  setBackground: (path: string | null) => invoke<ProjectView>("set_background", { path }),
  setComposite: (composite: Composite) => invoke<ProjectView>("set_composite", { composite }),
  exportVideo: () => invoke<ExportRecord>("export_video"),
  projectPath: (rel: string) => invoke<string>("project_path", { rel }),
};

export function onMaskProgress(cb: (done: number, total: number) => void): Promise<UnlistenFn> {
  return listen<{ done: number; total: number }>("mask-progress", (e) => cb(e.payload.done, e.payload.total));
}

export function onExportProgress(cb: (fraction: number) => void): Promise<UnlistenFn> {
  return listen<{ fraction: number }>("export-progress", (e) => cb(e.payload.fraction));
}

export const frameName = (i: number) => String(i).padStart(5, "0");
export const fileUrl = (path: string) => convertFileSrc(path);

export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}

export function formatTime(s: number): string {
  const m = Math.floor(s / 60);
  const sec = s - m * 60;
  return `${m}:${sec.toFixed(2).padStart(5, "0")}`;
}
