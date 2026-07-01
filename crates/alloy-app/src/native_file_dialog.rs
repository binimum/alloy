use std::path::PathBuf;
use std::process::Command;

pub fn pick_folder(title: &str) -> Result<Option<PathBuf>, String> {
    run_folder_picker(title).map(|output| first_path(&output))
}

pub fn pick_audio_files(title: &str) -> Result<Vec<PathBuf>, String> {
    run_file_picker(title).map(|output| paths_from_output(&output))
}

fn first_path(output: &str) -> Option<PathBuf> {
    paths_from_output(output).into_iter().next()
}

fn paths_from_output(output: &str) -> Vec<PathBuf> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[cfg(target_os = "macos")]
fn run_folder_picker(title: &str) -> Result<String, String> {
    let script = format!(r#"POSIX path of (choose folder with prompt "{}")"#, title);
    command_output(Command::new("osascript").args(["-e", &script]))
}

#[cfg(target_os = "macos")]
fn run_file_picker(title: &str) -> Result<String, String> {
    let script = format!(
        r#"
set selectedFiles to choose file with prompt "{}" with multiple selections allowed
set output to ""
repeat with selectedFile in selectedFiles
    set output to output & POSIX path of selectedFile & linefeed
end repeat
return output
"#,
        title
    );
    command_output(Command::new("osascript").args(["-e", &script]))
}

#[cfg(target_os = "windows")]
fn run_folder_picker(title: &str) -> Result<String, String> {
    let script = format!(
        r#"
Add-Type -AssemblyName System.Windows.Forms
$dialog = New-Object System.Windows.Forms.FolderBrowserDialog
$dialog.Description = '{}'
if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {{
    $dialog.SelectedPath
}}
"#,
        title.replace('\'', "''")
    );
    powershell_output(&script)
}

#[cfg(target_os = "windows")]
fn run_file_picker(title: &str) -> Result<String, String> {
    let script = format!(
        r#"
Add-Type -AssemblyName System.Windows.Forms
$dialog = New-Object System.Windows.Forms.OpenFileDialog
$dialog.Title = '{}'
$dialog.Multiselect = $true
$dialog.Filter = 'Audio files|*.flac;*.mp3;*.ogg;*.opus;*.wav;*.m4a;*.aac|All files|*.*'
if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {{
    $dialog.FileNames -join "`n"
}}
"#,
        title.replace('\'', "''")
    );
    powershell_output(&script)
}

#[cfg(target_os = "windows")]
fn powershell_output(script: &str) -> Result<String, String> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut command = Command::new("powershell.exe");
    command
        .creation_flags(CREATE_NO_WINDOW)
        .args(["-NoProfile", "-STA", "-Command", script]);
    command_output(&mut command)
}

#[cfg(target_os = "linux")]
fn run_folder_picker(title: &str) -> Result<String, String> {
    command_output(Command::new("zenity").args([
        "--file-selection",
        "--directory",
        "--title",
        title,
    ]))
    .or_else(|_| command_output(Command::new("kdialog").args(["--getexistingdirectory", "."])))
}

#[cfg(target_os = "linux")]
fn run_file_picker(title: &str) -> Result<String, String> {
    command_output(Command::new("zenity").args([
        "--file-selection",
        "--multiple",
        "--separator=\n",
        "--title",
        title,
        "--file-filter=Audio files | *.flac *.mp3 *.ogg *.opus *.wav *.m4a *.aac",
    ]))
    .or_else(|_| {
        command_output(Command::new("kdialog").args([
            "--multiple",
            "--separate-output",
            "--getopenfilename",
            ".",
            "Audio files (*.flac *.mp3 *.ogg *.opus *.wav *.m4a *.aac)",
        ]))
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn run_folder_picker(_title: &str) -> Result<String, String> {
    Err("native file picker is not supported on this platform".to_owned())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn run_file_picker(_title: &str) -> Result<String, String> {
    Err("native file picker is not supported on this platform".to_owned())
}

fn command_output(command: &mut Command) -> Result<String, String> {
    let output = command
        .output()
        .map_err(|err| format!("could not open native file picker: {err}"))?;

    if output.status.success() {
        return String::from_utf8(output.stdout)
            .map_err(|err| format!("file picker returned invalid text: {err}"));
    }

    if output.status.code().is_none() {
        return Ok(String::new());
    }

    let error = String::from_utf8_lossy(&output.stderr);
    if error.trim().is_empty() {
        Err("native file picker was cancelled or failed".to_owned())
    } else {
        Err(error.trim().to_owned())
    }
}
