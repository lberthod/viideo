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
  edge: number; temperature: number; bg_blur: number;
}

export interface GenerationSettings { backend: string; comfy_url: string }

export interface BackgroundItem {
  id: string; path: string; abs_path: string; source: "imported" | "generated"; created_at: string;
  meta: { prompt?: string; seed?: number; model?: string; backend?: string; workflow?: string; workflow_version?: number;
          workflow_tested?: boolean; license?: string; seconds?: number; width?: number; height?: number; original_name?: string };
}

export interface WorkflowStatus { id: string; version: number; title: string; description: string; tested: boolean; ready: boolean; problems: string[] }
export interface ImageBackends {
  builtin_ok: boolean; builtin_detail: string; settings: GenerationSettings;
  comfy: { url: string; reachable: boolean; version: string | null; detail: string; workflows: WorkflowStatus[] };
}

export type ExportQuality = "fast" | "balanced" | "high";

export interface FrameQuality {
  frame: number; area_pct: number; components: number; iou_prev: number; cx: number; cy: number;
  islands_removed: number; holes_filled: number;
}
export interface FlaggedFrame { frame: number; score: number; reasons: string[] }
export interface MaskQuality {
  summary: { frames: number; empty_frames: number; iou_mean: number; iou_min: number; flagged: number; islands_removed: number; holes_filled: number; warnings?: number };
  warnings?: string[];
  flagged: FlaggedFrame[];
  frames: FrameQuality[];
}

export interface DiskUsage {
  proxy_current: number; proxy_stale: number; masks_current: number; masks_stale: number;
  previews: number; assets: number; exports: number;
}

export interface ExportRecord { path: string; created_at: string; seconds: number; width: number; height: number; size_bytes: number }

export interface Manifest {
  schema_version: number; app_version: string; created_at: string; updated_at: string;
  source: { path: string; fingerprint: string; probe: Probe };
  clip: { start: number; duration: number };
  proxy: { key: string; dir: string; width: number; height: number; frames: number; fps_num: number; fps_den: number } | null;
  segmentation: {
    model: string; dtype: string; prompts: Prompt[]; status: "none" | "stale" | "ready"; cleanup: boolean;
    mask: { key: string; dir: string; frames: number; seconds: number; empty_frames: number; flagged: number } | null;
  };
  composite: Composite;
  exports: ExportRecord[];
  export_settings: { quality: ExportQuality };
  generation: GenerationSettings;
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
  maskQuality: () => invoke<MaskQuality | null>("mask_quality"),
  setCleanup: (enabled: boolean) => invoke<ProjectView>("set_cleanup", { enabled }),
  setExportQuality: (quality: ExportQuality) => invoke<ProjectView>("set_export_quality", { quality }),
  diskUsage: () => invoke<DiskUsage>("disk_usage"),
  cleanCache: () => invoke<DiskUsage>("clean_cache"),
  listBackgrounds: () => invoke<BackgroundItem[]>("list_backgrounds"),
  selectBackground: (rel: string) => invoke<ProjectView>("select_background", { rel }),
  deleteBackground: (rel: string) => invoke<BackgroundItem[]>("delete_background", { rel }),
  imageBackends: () => invoke<ImageBackends>("image_backends"),
  setGeneration: (settings: GenerationSettings) => invoke<ProjectView>("set_generation", { settings }),
  generateBackgrounds: (prompt: string, count: number, seed: number | null) =>
    invoke<BackgroundItem[]>("generate_backgrounds", { prompt, count, seed }),
};

export function onMaskProgress(cb: (done: number, total: number) => void): Promise<UnlistenFn> {
  return listen<{ done: number; total: number }>("mask-progress", (e) => cb(e.payload.done, e.payload.total));
}

export function onImageProgress(cb: (p: { index?: number; count?: number; done: number; total: number }) => void): Promise<UnlistenFn> {
  return listen<{ index?: number; count?: number; done: number; total: number }>("image-progress", (e) => cb(e.payload));
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

export function formatBytes(n: number): string {
  if (n < 1e6) return `${(n / 1e3).toFixed(0)} Ko`;
  if (n < 1e9) return `${(n / 1e6).toFixed(1)} Mo`;
  return `${(n / 1e9).toFixed(2)} Go`;
}
