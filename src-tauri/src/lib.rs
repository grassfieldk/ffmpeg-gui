use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::Emitter;
use tauri::Manager;
use zip::ZipArchive;

const FFMPEG_VERSION: &str = "8.0.1";
const FFMPEG_ZIP_URL: &str = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip";
const FFMPEG_ZIP_SHA256: &str = "e2aaeaa0fdbc397d4794828086424d4aaa2102cef1fb6874f6ffd29c0b88b673";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FfmpegReady {
    source: String,
    ffmpeg_path: String,
    version: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConvertPreview {
    output_path: String,
    args: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConvertResult {
    output_path: String,
    exit_code: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CancelResult {
    requested: bool,
}

struct AppState {
    current_pid: Mutex<Option<u32>>,
    cancel_requested: AtomicBool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConvertOptions {
    width: u32,
    height: u32,
    video_bitrate_k: u32,
    fps_mode: String,
    frame_rate: f32,
    audio_format: String,
    audio_bitrate_k: u32,
    crf: u8,
    output_ext: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeResult {
    width: u32,
    height: u32,
    frame_rate: f32,
    video_bitrate_k: u32,
    audio_bitrate_k: u32,
    audio_format: String,
    duration_sec: f32,
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn parse_fps(value: &str) -> f32 {
    if let Some((a, b)) = value.split_once('/') {
        let numerator = a.parse::<f32>().unwrap_or(0.0);
        let denominator = b.parse::<f32>().unwrap_or(1.0);
        if denominator > 0.0 {
            return numerator / denominator;
        }
    }
    value.parse::<f32>().unwrap_or(0.0)
}

fn build_output_path(input_path: &Path, ext: &str) -> Result<PathBuf, String> {
    let parent = input_path
        .parent()
        .ok_or_else(|| "入力ファイルの親フォルダを取得できません".to_string())?;
    let stem = input_path
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "入力ファイル名を解決できません".to_string())?;
    Ok(parent.join(format!("{}_converted.{}", stem, ext.to_lowercase())))
}

fn build_ffmpeg_args(input_path: &Path, output_path: &Path, options: &ConvertOptions) -> Vec<String> {
    let mut args = vec![
        "-y".to_string(),
        "-i".to_string(),
        input_path.display().to_string(),
        "-vf".to_string(),
        format!("scale={}:{}", options.width, options.height),
        "-b:v".to_string(),
        format!("{}k", options.video_bitrate_k),
        "-crf".to_string(),
        options.crf.to_string(),
        "-c:a".to_string(),
        options.audio_format.clone(),
        "-b:a".to_string(),
        format!("{}k", options.audio_bitrate_k),
    ];

    if options.fps_mode == "fixed" {
        args.push("-r".to_string());
        args.push(options.frame_rate.to_string());
        args.push("-fps_mode".to_string());
        args.push("cfr".to_string());
    } else {
        args.push("-fps_mode".to_string());
        args.push("vfr".to_string());
    }

    args.push(output_path.display().to_string());
    args
}

fn run_ffprobe(ffprobe_executable: &Path, input_path: &Path) -> Result<ProbeResult, String> {
    let output = Command::new(ffprobe_executable)
        .arg("-v")
        .arg("error")
        .arg("-show_streams")
        .arg("-show_format")
        .arg("-of")
        .arg("json")
        .arg(input_path)
        .output()
        .map_err(|error| format!("ffprobe 実行失敗: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "ffprobe が失敗しました: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|error| format!("JSON解析失敗: {error}"))?;

    let streams = value
        .get("streams")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "ffprobe streams が取得できません".to_string())?;

    let mut width = 0;
    let mut height = 0;
    let mut frame_rate = 0.0;
    let mut video_bitrate_k = 0;
    let mut audio_bitrate_k = 0;
    let mut audio_format = "aac".to_string();
    let mut duration_sec = 0.0;

    for stream in streams {
        let codec_type = stream
            .get("codec_type")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if codec_type == "video" {
            width = stream.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            height = stream.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            frame_rate = parse_fps(
                stream
                    .get("avg_frame_rate")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0/1"),
            );
            video_bitrate_k = stream
                .get("bit_rate")
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse::<u64>().ok())
                .map(|v| (v / 1000) as u32)
                .unwrap_or(0);
        }
        if codec_type == "audio" {
            audio_bitrate_k = stream
                .get("bit_rate")
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse::<u64>().ok())
                .map(|v| (v / 1000) as u32)
                .unwrap_or(0);
            audio_format = stream
                .get("codec_name")
                .and_then(|v| v.as_str())
                .unwrap_or("aac")
                .to_string();
        }
    }

    if let Some(value) = value
        .get("format")
        .and_then(|v| v.get("duration"))
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse::<f32>().ok())
    {
        duration_sec = value;
    }

    Ok(ProbeResult {
        width,
        height,
        frame_rate,
        video_bitrate_k,
        audio_bitrate_k,
        audio_format,
        duration_sec,
    })
}

fn find_ffmpeg_on_path() -> Option<PathBuf> {
    let output = Command::new("ffmpeg").arg("-version").output().ok()?;
    if output.status.success() {
        Some(PathBuf::from("ffmpeg"))
    } else {
        None
    }
}

fn find_ffprobe_on_path() -> Option<PathBuf> {
    let output = Command::new("ffprobe").arg("-version").output().ok()?;
    if output.status.success() {
        Some(PathBuf::from("ffprobe"))
    } else {
        None
    }
}

fn kill_process_by_pid(pid: u32) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let status = Command::new("taskkill")
            .arg("/PID")
            .arg(pid.to_string())
            .arg("/T")
            .arg("/F")
            .status()
            .map_err(|error| format!("ERR_CANCEL: taskkill 実行失敗: {error}"))?;
        if status.success() {
            return Ok(());
        }
        return Err(format!("ERR_CANCEL: taskkill 失敗 status={status}"));
    }

    #[cfg(not(target_os = "windows"))]
    {
        let status = Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status()
            .map_err(|error| format!("ERR_CANCEL: kill 実行失敗: {error}"))?;
        if status.success() {
            return Ok(());
        }
        Err(format!("ERR_CANCEL: kill 失敗 status={status}"))
    }
}

fn resolve_ffmpeg_executable_path(app: &tauri::AppHandle, ffmpeg_path: &str) -> Result<PathBuf, String> {
    if ffmpeg_path == "ffmpeg" {
        return Ok(PathBuf::from("ffmpeg"));
    }

    let executable = PathBuf::from(ffmpeg_path);
    if executable.exists() {
        return Ok(executable);
    }

    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("app_data_dir 解決失敗: {error}"))?;
    let bundled = app_data
        .join("ffmpeg")
        .join(FFMPEG_VERSION)
        .join("bin")
        .join("ffmpeg.exe");
    if bundled.exists() {
        return Ok(bundled);
    }

    find_ffmpeg_on_path().ok_or_else(|| "ERR_FFMPEG_NOT_FOUND: ffmpeg が見つかりません".to_string())
}

async fn download_ffmpeg_zip() -> Result<Vec<u8>, String> {
    let client = Client::new();
    let response = client
        .get(FFMPEG_ZIP_URL)
        .send()
        .await
        .map_err(|error| format!("ERR_DOWNLOAD: FFmpeg ダウンロード失敗: {error}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("ERR_DOWNLOAD: FFmpeg ダウンロードに失敗しました: status={status}"));
    }

    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|error| format!("ERR_DOWNLOAD: FFmpeg ダウンロード結果の取得失敗: {error}"))
}

fn validate_sha256(bytes: &[u8], expected_hash: &str) -> Result<(), String> {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected_hash {
        return Err(format!(
            "ERR_HASH: SHA-256 不一致: expected={expected_hash}, actual={actual}"
        ));
    }
    Ok(())
}

fn extract_executable_from_zip(
    zip_bytes: &[u8],
    destination_dir: &Path,
    executable_name: &str,
) -> Result<PathBuf, String> {
    let reader = Cursor::new(zip_bytes);
    let mut archive =
        ZipArchive::new(reader).map_err(|error| format!("ERR_EXTRACT: ZIP展開失敗: {error}"))?;
    for index in 0..archive.len() {
        let mut item = archive
            .by_index(index)
            .map_err(|error| format!("ERR_EXTRACT: ZIPエントリ取得失敗: {error}"))?;
        let item_name = item.name().to_lowercase();
        if item_name.ends_with(&format!("/bin/{executable_name}")) {
            let destination_path = destination_dir.join(executable_name);
            let mut output = File::create(&destination_path)
                .map_err(|error| format!("ERR_EXTRACT: 展開先作成失敗: {error}"))?;
            let mut buffer = Vec::new();
            item.read_to_end(&mut buffer)
                .map_err(|error| format!("ERR_EXTRACT: ZIP読み取り失敗: {error}"))?;
            output
                .write_all(&buffer)
                .map_err(|error| format!("ERR_EXTRACT: ZIP書き出し失敗: {error}"))?;
            return Ok(destination_path);
        }
    }
    Err(format!("ERR_EXTRACT: ZIP内に {executable_name} が見つかりません"))
}

async fn ensure_ffmpeg_internal(app: &tauri::AppHandle) -> Result<FfmpegReady, String> {
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("app_data_dir 解決失敗: {error}"))?;
    let ffmpeg_root = app_data.join("ffmpeg").join(FFMPEG_VERSION).join("bin");
    let ffmpeg_executable = ffmpeg_root.join("ffmpeg.exe");

    if ffmpeg_executable.exists() {
        return Ok(FfmpegReady {
            source: "downloaded".to_string(),
            ffmpeg_path: normalize_path(ffmpeg_executable.to_string_lossy().as_ref()),
            version: FFMPEG_VERSION.to_string(),
        });
    }

    if let Some(path_ffmpeg) = find_ffmpeg_on_path() {
        return Ok(FfmpegReady {
            source: "path".to_string(),
            ffmpeg_path: path_ffmpeg.to_string_lossy().to_string(),
            version: "external".to_string(),
        });
    }

    fs::create_dir_all(&ffmpeg_root).map_err(|error| format!("作業フォルダ作成失敗: {error}"))?;
    let zip_bytes = download_ffmpeg_zip().await?;
    validate_sha256(&zip_bytes, FFMPEG_ZIP_SHA256)?;

    extract_executable_from_zip(&zip_bytes, &ffmpeg_root, "ffmpeg.exe")?;
    extract_executable_from_zip(&zip_bytes, &ffmpeg_root, "ffprobe.exe")?;

    Ok(FfmpegReady {
        source: "downloaded".to_string(),
        ffmpeg_path: normalize_path(ffmpeg_executable.to_string_lossy().as_ref()),
        version: FFMPEG_VERSION.to_string(),
    })
}

fn resolve_ffprobe_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("app_data_dir 解決失敗: {error}"))?;
    let bundled = app_data
        .join("ffmpeg")
        .join(FFMPEG_VERSION)
        .join("bin")
        .join("ffprobe.exe");
    if bundled.exists() {
        return Ok(bundled);
    }
    find_ffprobe_on_path().ok_or_else(|| "ERR_FFMPEG_NOT_FOUND: ffprobe が見つかりません".to_string())
}

#[tauri::command]
async fn ensure_ffmpeg_ready(app: tauri::AppHandle) -> Result<FfmpegReady, String> {
    ensure_ffmpeg_internal(&app).await
}

#[tauri::command]
async fn probe_video(app: tauri::AppHandle, input_path: String) -> Result<ProbeResult, String> {
    let input = PathBuf::from(input_path);
    if !input.exists() {
        return Err("入力ファイルが存在しません".to_string());
    }
    let ffprobe = resolve_ffprobe_path(&app)?;
    run_ffprobe(&ffprobe, &input)
}

#[tauri::command]
async fn preview_convert_command(
    input_path: String,
    options: ConvertOptions,
) -> Result<ConvertPreview, String> {
    let input = PathBuf::from(input_path);
    if !input.exists() {
        return Err("入力ファイルが存在しません".to_string());
    }
    let output = build_output_path(&input, &options.output_ext)?;
    let args = build_ffmpeg_args(&input, &output, &options);
    Ok(ConvertPreview {
        output_path: normalize_path(output.to_string_lossy().as_ref()),
        args,
    })
}

#[tauri::command]
async fn run_convert(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    input_path: String,
    options: ConvertOptions,
) -> Result<ConvertResult, String> {
    let input = PathBuf::from(input_path);
    if !input.exists() {
        return Err("入力ファイルが存在しません".to_string());
    }

    let ready = ensure_ffmpeg_internal(&app).await?;
    let ffmpeg_executable = resolve_ffmpeg_executable_path(&app, &ready.ffmpeg_path)?;
    let output = build_output_path(&input, &options.output_ext)?;
    let args = build_ffmpeg_args(&input, &output, &options);

    {
        let running = state
            .current_pid
            .lock()
            .map_err(|_| "ERR_STATE: state lock失敗".to_string())?;
        if running.is_some() {
            return Err("ERR_BUSY: すでに変換中です".to_string());
        }
    }

    state.cancel_requested.store(false, Ordering::SeqCst);

    app.emit(
        "convert-log",
        serde_json::json!({
            "message": format!("実行開始: {}", ffmpeg_executable.display())
        }),
    )
    .map_err(|error| format!("ログ送信失敗: {error}"))?;

    let mut child = Command::new(&ffmpeg_executable)
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("ERR_START: ffmpeg 起動失敗: {error}"))?;

    {
        let mut running = state
            .current_pid
            .lock()
            .map_err(|_| "ERR_STATE: state lock失敗".to_string())?;
        *running = Some(child.id());
    }
    let execution_result: Result<ConvertResult, String> = (|| {
        if let Some(stderr) = child.stderr.take() {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(text) if !text.trim().is_empty() => {
                        app.emit("convert-log", serde_json::json!({ "message": text }))
                            .map_err(|error| format!("ERR_LOG: ログ送信失敗: {error}"))?;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        return Err(format!("ERR_LOG: ログ読み取り失敗: {error}"));
                    }
                }
            }
        }

        let status = child
            .wait()
            .map_err(|error| format!("ERR_WAIT: ffmpeg wait失敗: {error}"))?;

        let exit_code = status.code().unwrap_or(-1);
        if state.cancel_requested.load(Ordering::SeqCst) {
            return Err("ERR_CANCELLED: 変換はユーザーによりキャンセルされました".to_string());
        }
        if !status.success() {
            return Err(format!("ERR_CONVERT: ffmpeg 失敗: exit={exit_code}"));
        }

        app.emit(
            "convert-log",
            serde_json::json!({
                "message": format!("変換成功: {}", output.display())
            }),
        )
        .map_err(|error| format!("ERR_LOG: ログ送信失敗: {error}"))?;

        Ok(ConvertResult {
            output_path: normalize_path(output.to_string_lossy().as_ref()),
            exit_code,
        })
    })();

    {
        let mut running = state
            .current_pid
            .lock()
            .map_err(|_| "ERR_STATE: state lock失敗".to_string())?;
        *running = None;
    }

    execution_result
}

#[tauri::command]
async fn cancel_convert(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<CancelResult, String> {
    let pid = {
        let running = state
            .current_pid
            .lock()
            .map_err(|_| "ERR_STATE: state lock失敗".to_string())?;
        *running
    };

    if let Some(pid) = pid {
        state.cancel_requested.store(true, Ordering::SeqCst);
        kill_process_by_pid(pid)?;
        app.emit(
            "convert-log",
            serde_json::json!({ "message": format!("キャンセル要求送信: pid={pid}") }),
        )
        .map_err(|error| format!("ERR_LOG: ログ送信失敗: {error}"))?;
        return Ok(CancelResult { requested: true });
    }

    Ok(CancelResult { requested: false })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            current_pid: Mutex::new(None),
            cancel_requested: AtomicBool::new(false),
        })
        .invoke_handler(tauri::generate_handler![
            ensure_ffmpeg_ready,
            probe_video,
            preview_convert_command,
            run_convert,
            cancel_convert
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
