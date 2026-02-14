import "./style.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type ProbeResult = {
  width: number;
  height: number;
  frameRate: number;
  videoBitrateK: number;
  audioBitrateK: number;
  audioFormat: string;
  durationSec: number;
};

type ConvertOptions = {
  width: number;
  height: number;
  videoBitrateK: number;
  fpsMode: "fixed" | "variable";
  frameRate: number;
  audioFormat: string;
  audioBitrateK: number;
  crf: number;
  outputExt: string;
};

const appState: {
  inputPath: string | null;
  defaults: ConvertOptions | null;
  converting: boolean;
  durationSec: number;
} = {
  inputPath: null,
  defaults: null,
  converting: false,
  durationSec: 0
};

const resolutionMap: Record<string, { width: number; height: number }> = {
  "480-landscape": { width: 854, height: 480 },
  "480-portrait": { width: 480, height: 854 },
  "720-landscape": { width: 1280, height: 720 },
  "720-portrait": { width: 720, height: 1280 },
  "1080-landscape": { width: 1920, height: 1080 },
  "1080-portrait": { width: 1080, height: 1920 }
};

const elements = {
  dropZone: document.querySelector<HTMLLabelElement>("#drop-zone")!,
  fileInput: document.querySelector<HTMLInputElement>("#file-input")!,
  fileName: document.querySelector<HTMLParagraphElement>("#file-name")!,
  preset: document.querySelector<HTMLSelectElement>("#preset")!,
  resolutionTemplate: document.querySelector<HTMLSelectElement>("#resolution-template")!,
  width: document.querySelector<HTMLInputElement>("#width")!,
  height: document.querySelector<HTMLInputElement>("#height")!,
  videoBitrate: document.querySelector<HTMLInputElement>("#video-bitrate")!,
  fpsMode: document.querySelector<HTMLSelectElement>("#fps-mode")!,
  frameRate: document.querySelector<HTMLInputElement>("#framerate")!,
  audioFormat: document.querySelector<HTMLSelectElement>("#audio-format")!,
  audioBitrate: document.querySelector<HTMLInputElement>("#audio-bitrate")!,
  crf: document.querySelector<HTMLInputElement>("#crf")!,
  extension: document.querySelector<HTMLInputElement>("#extension")!,
  setupButton: document.querySelector<HTMLButtonElement>("#setup-button")!,
  previewButton: document.querySelector<HTMLButtonElement>("#preview-button")!,
  convertButton: document.querySelector<HTMLButtonElement>("#convert-button")!,
  cancelButton: document.querySelector<HTMLButtonElement>("#cancel-button")!,
  progress: document.querySelector<HTMLProgressElement>("#progress")!,
  progressText: document.querySelector<HTMLSpanElement>("#progress-text")!,
  log: document.querySelector<HTMLPreElement>("#log")!
};

function appendLog(message: string): void {
  const now = new Date().toLocaleTimeString();
  elements.log.textContent += `[${now}] ${message}\n`;
  elements.log.scrollTop = elements.log.scrollHeight;
}

function setConvertingState(value: boolean): void {
  appState.converting = value;
  elements.convertButton.disabled = value;
  elements.previewButton.disabled = value;
  elements.setupButton.disabled = value;
  elements.cancelButton.disabled = !value;
}

function setProgress(value: number): void {
  const safeValue = Math.max(0, Math.min(100, value));
  elements.progress.value = safeValue;
  elements.progressText.textContent = `${safeValue.toFixed(1)}%`;
}

function parseTimestampToSeconds(value: string): number {
  const parts = value.split(":");
  if (parts.length !== 3) {
    return 0;
  }
  const hours = Number(parts[0]);
  const minutes = Number(parts[1]);
  const seconds = Number(parts[2]);
  if ([hours, minutes, seconds].some((v) => Number.isNaN(v))) {
    return 0;
  }
  return hours * 3600 + minutes * 60 + seconds;
}

function updateProgressFromLogLine(line: string): void {
  if (!appState.durationSec || appState.durationSec <= 0) {
    return;
  }
  const match = line.match(/time=(\d{2}:\d{2}:\d{2}(?:\.\d+)?)/);
  if (!match) {
    return;
  }
  const elapsed = parseTimestampToSeconds(match[1]);
  if (elapsed <= 0) {
    return;
  }
  const ratio = (elapsed / appState.durationSec) * 100;
  setProgress(ratio);
}

function formatBackendError(error: unknown): string {
  const raw = String(error);
  if (raw.includes("ERR_DOWNLOAD")) {
    return "FFmpeg ダウンロードに失敗しました。ネットワーク接続を確認してください。";
  }
  if (raw.includes("ERR_HASH")) {
    return "FFmpeg ダウンロードファイルの検証に失敗しました（SHA-256不一致）。";
  }
  if (raw.includes("ERR_FFMPEG_NOT_FOUND")) {
    return "FFmpeg が見つかりません。ネット接続またはPATH設定を確認してください。";
  }
  if (raw.includes("ERR_CANCELLED")) {
    return "変換をキャンセルしました。";
  }
  if (raw.includes("ERR_START")) {
    return "FFmpeg の起動に失敗しました。";
  }
  return raw;
}

function getCurrentOptions(): ConvertOptions {
  return {
    width: Number(elements.width.value),
    height: Number(elements.height.value),
    videoBitrateK: Number(elements.videoBitrate.value),
    fpsMode: elements.fpsMode.value as "fixed" | "variable",
    frameRate: Number(elements.frameRate.value),
    audioFormat: elements.audioFormat.value,
    audioBitrateK: Number(elements.audioBitrate.value),
    crf: Number(elements.crf.value),
    outputExt: elements.extension.value
  };
}

function setOptions(options: ConvertOptions): void {
  elements.width.value = String(options.width);
  elements.height.value = String(options.height);
  elements.videoBitrate.value = String(options.videoBitrateK);
  elements.fpsMode.value = options.fpsMode;
  elements.frameRate.value = String(options.frameRate);
  elements.audioFormat.value = options.audioFormat;
  elements.audioBitrate.value = String(options.audioBitrateK);
  elements.crf.value = String(options.crf);
  elements.extension.value = options.outputExt.toLowerCase();
}

function applyPreset(preset: string): void {
  if (!appState.defaults) {
    return;
  }

  const next: ConvertOptions = { ...getCurrentOptions() };
  if (preset === "edit") {
    next.fpsMode = "fixed";
    next.audioFormat = "aac";
    next.outputExt = "mp4";
  }
  if (preset === "sns") {
    next.width = 854;
    next.height = 480;
    next.crf = 28;
    next.fpsMode = "fixed";
    next.frameRate = 30;
    next.audioFormat = "aac";
    next.outputExt = "mp4";
  }
  if (preset === "custom") {
    Object.assign(next, appState.defaults);
  }
  setOptions(next);
}

async function probeAndFillDefaults(filePath: string): Promise<void> {
  const probe = await invoke<ProbeResult>("probe_video", { inputPath: filePath });
  appState.durationSec = probe.durationSec || 0;
  const defaults: ConvertOptions = {
    width: probe.width,
    height: probe.height,
    videoBitrateK: probe.videoBitrateK || 3000,
    fpsMode: "variable",
    frameRate: probe.frameRate || 30,
    audioFormat: probe.audioFormat || "aac",
    audioBitrateK: probe.audioBitrateK || 128,
    crf: 23,
    outputExt: "mp4"
  };
  appState.defaults = defaults;
  setOptions(defaults);
}

async function handleFilePath(filePath: string): Promise<void> {
  appState.inputPath = filePath;
  elements.fileName.textContent = filePath;
  setProgress(0);
  appendLog(`入力ファイル: ${filePath}`);
  await probeAndFillDefaults(filePath);
  appendLog("ffprobe で初期値を反映しました");
}

elements.fileInput.addEventListener("change", async (event) => {
  const target = event.target as HTMLInputElement;
  if ((target.files?.length ?? 0) > 1) {
    appendLog("入力は1ファイルのみです");
    return;
  }
  const file = target.files?.[0];
  if (!file) {
    return;
  }
  const path = (file as File & { path?: string }).path ?? file.name;
  try {
    await handleFilePath(path);
  } catch (error) {
    appendLog(`ファイル読み込みエラー: ${String(error)}`);
  }
});

elements.dropZone.addEventListener("dragover", (event) => {
  event.preventDefault();
  elements.dropZone.classList.add("dragover");
});

elements.dropZone.addEventListener("dragleave", () => {
  elements.dropZone.classList.remove("dragover");
});

elements.dropZone.addEventListener("drop", async (event) => {
  event.preventDefault();
  elements.dropZone.classList.remove("dragover");
  const count = event.dataTransfer?.files?.length ?? 0;
  if (count > 1) {
    appendLog("入力は1ファイルのみです。先頭ファイルだけ受け付けます。");
  }
  const file = event.dataTransfer?.files?.[0];
  if (!file) {
    return;
  }
  const path = (file as File & { path?: string }).path ?? file.name;
  try {
    await handleFilePath(path);
  } catch (error) {
    appendLog(`ドロップ処理エラー: ${String(error)}`);
  }
});

elements.preset.addEventListener("change", () => {
  applyPreset(elements.preset.value);
  appendLog(`プリセット適用: ${elements.preset.value}`);
});

elements.resolutionTemplate.addEventListener("change", () => {
  const value = elements.resolutionTemplate.value;
  const resolved = resolutionMap[value];
  if (!resolved) {
    return;
  }
  elements.width.value = String(resolved.width);
  elements.height.value = String(resolved.height);
  appendLog(`解像度テンプレート適用: ${value}`);
});

document.querySelectorAll<HTMLButtonElement>(".reset").forEach((button) => {
  button.addEventListener("click", () => {
    if (!appState.defaults) {
      return;
    }
    const key = button.dataset.key as keyof ConvertOptions;
    const options = getCurrentOptions();
    options[key] = appState.defaults[key] as never;
    setOptions(options);
    appendLog(`項目リセット: ${key}`);
  });
});

elements.setupButton.addEventListener("click", async () => {
  try {
    appendLog("FFmpeg 準備を開始します");
    const result = await invoke<{ source: string; ffmpegPath: string; version: string }>(
      "ensure_ffmpeg_ready"
    );
    appendLog(`FFmpeg 準備完了: source=${result.source}, version=${result.version}`);
    appendLog(`ffmpeg path: ${result.ffmpegPath}`);
  } catch (error) {
    appendLog(`FFmpeg 準備失敗: ${formatBackendError(error)}`);
  }
});

elements.previewButton.addEventListener("click", async () => {
  if (!appState.inputPath) {
    appendLog("入力ファイルを選択してください");
    return;
  }
  try {
    const preview = await invoke<{ outputPath: string; args: string[] }>("preview_convert_command", {
      inputPath: appState.inputPath,
      options: getCurrentOptions()
    });
    appendLog(`出力先: ${preview.outputPath}`);
    appendLog(`ffmpeg ${preview.args.join(" ")}`);
  } catch (error) {
    appendLog(`コマンド生成失敗: ${formatBackendError(error)}`);
  }
});

elements.convertButton.addEventListener("click", async () => {
  if (!appState.inputPath) {
    appendLog("入力ファイルを選択してください");
    return;
  }
  if (appState.converting) {
    appendLog("変換中です");
    return;
  }
  try {
    setConvertingState(true);
    setProgress(0);
    appendLog("変換を開始します");
    const result = await invoke<{ outputPath: string; exitCode: number }>("run_convert", {
      inputPath: appState.inputPath,
      options: getCurrentOptions()
    });
    setProgress(100);
    appendLog(`変換完了: ${result.outputPath} (exit=${result.exitCode})`);
  } catch (error) {
    appendLog(`変換失敗: ${formatBackendError(error)}`);
    if (String(error).includes("ERR_CANCELLED")) {
      setProgress(0);
    }
  } finally {
    setConvertingState(false);
  }
});

elements.cancelButton.addEventListener("click", async () => {
  if (!appState.converting) {
    appendLog("現在は変換中ではありません");
    return;
  }
  try {
    const result = await invoke<{ requested: boolean }>("cancel_convert");
    if (result.requested) {
      appendLog("キャンセル要求を送信しました");
    } else {
      appendLog("キャンセル対象のプロセスが見つかりません");
    }
  } catch (error) {
    appendLog(`キャンセル失敗: ${formatBackendError(error)}`);
  }
});

await listen<{ message: string }>("convert-log", (event) => {
  if (event.payload?.message) {
    updateProgressFromLogLine(event.payload.message);
    appendLog(event.payload.message);
  }
});

setConvertingState(false);
appendLog("アプリ起動");
