//! Gerenciamento explícito de dependências Python por projeto.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CONFIG_FILE: &str = "python-projects.json";

fn defaults() -> Value { json!({"installer":"pip","useVenv":true}) }

/// Carrega preferências Python persistidas sem aceitar valores desconhecidos.
pub(crate) fn load_settings(data_dir: &Path) -> Value {
    let saved = fs::read(data_dir.join(CONFIG_FILE)).ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok()).unwrap_or_else(defaults);
    let installer = saved["installer"].as_str().filter(|v| matches!(*v, "pip" | "uv")).unwrap_or("pip");
    json!({"installer":installer,"useVenv":saved["useVenv"].as_bool().unwrap_or(true)})
}

/// Salva a escolha global de instalador e isolamento por projeto.
pub(crate) fn save_settings(data_dir: &Path, body: &Value) -> Result<Value, String> {
    let installer = body["installer"].as_str().filter(|v| matches!(*v, "pip" | "uv"))
        .ok_or_else(|| "Choose pip or uv.".to_string())?;
    let value = json!({"installer":installer,"useVenv":body["useVenv"].as_bool().unwrap_or(true)});
    fs::create_dir_all(data_dir).map_err(|_| "Python settings directory could not be created.".to_string())?;
    let target = data_dir.join(CONFIG_FILE);
    let temporary = data_dir.join(format!("{CONFIG_FILE}.tmp"));
    fs::write(&temporary, serde_json::to_vec_pretty(&value).unwrap())
        .and_then(|_| fs::rename(&temporary, &target))
        .map_err(|_| "Python settings could not be saved.".to_string())?;
    Ok(value)
}

fn command_works(program: &Path, args: &[&str]) -> bool {
    Command::new(program).args(args).output().is_ok_and(|output| output.status.success())
}

fn find_python() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("QUIXI_TEST_PYTHON") {
        let path = PathBuf::from(value);
        if command_works(&path, &["--version"]) { return Some(path); }
    }
    ["python3", "python", "py"].iter().map(PathBuf::from)
        .find(|candidate| command_works(candidate, &["--version"]))
}

fn project_path(value: &Value) -> Result<PathBuf, String> {
    let raw = value["path"].as_str().unwrap_or("");
    if raw.trim().is_empty() { return Err("Choose a project folder first.".into()); }
    let path = fs::canonicalize(raw).map_err(|_| "Project folder was not found.".to_string())?;
    if !path.is_dir() { return Err("The selected project path is not a folder.".into()); }
    Ok(path)
}

fn venv_python(project: &Path) -> PathBuf {
    if cfg!(windows) { project.join(".venv").join("Scripts").join("python.exe") }
    else { project.join(".venv").join("bin").join("python") }
}

fn run(program: &Path, args: &[&str], cwd: &Path) -> Result<Output, String> {
    Command::new(program).args(args).current_dir(cwd).output()
        .map_err(|_| "The Python command could not be started.".to_string())
}

fn output_error(output: &Output, fallback: &str) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = stderr.lines().find(|line| !line.trim().is_empty()).unwrap_or(fallback);
    line.chars().take(500).collect()
}

/// Garante o interpretador correto, criando `.venv` somente quando habilitado.
fn selected_python(project: &Path, use_venv: bool) -> Result<PathBuf, String> {
    let system = find_python().ok_or_else(|| "Python was not found. Install Python before managing dependencies.".to_string())?;
    if !use_venv { return Ok(system); }
    let interpreter = venv_python(project);
    if interpreter.is_file() { return Ok(interpreter); }
    let output = run(&system, &["-m", "venv", ".venv"], project)?;
    if !output.status.success() { return Err(output_error(&output, "The virtual environment could not be created.")); }
    if !interpreter.is_file() { return Err("Python completed without creating the expected virtual environment.".into()); }
    Ok(interpreter)
}

fn tool_available(installer: &str, python: &Path, project: &Path) -> bool {
    if installer == "pip" { return run(python, &["-m", "pip", "--version"], project).is_ok_and(|o| o.status.success()); }
    Command::new("uv").arg("--version").current_dir(project).output().is_ok_and(|o| o.status.success())
        || run(python, &["-m", "uv", "--version"], project).is_ok_and(|o| o.status.success())
}

/// Informa disponibilidade sem instalar ferramentas nem criar arquivos.
pub(crate) fn status(data_dir: &Path, body: &Value) -> Result<Value, String> {
    let settings = load_settings(data_dir);
    let project = project_path(body)?;
    let python = find_python();
    let installer = settings["installer"].as_str().unwrap_or("pip");
    let available = python.as_ref().is_some_and(|p| tool_available(installer, p, &project));
    Ok(json!({"pythonAvailable":python.is_some(),"toolAvailable":available,"installer":installer,
        "useVenv":settings["useVenv"],"venvExists":venv_python(&project).is_file(),
        "requirementsExists":project.join("requirements.txt").is_file()}))
}

/// Instala pip via ensurepip ou uv via o Python somente após confirmação da interface.
pub(crate) fn install_tool(data_dir: &Path, body: &Value) -> Result<Value, String> {
    let settings = load_settings(data_dir);
    let project = project_path(body)?;
    let installer = settings["installer"].as_str().unwrap_or("pip");
    let python = selected_python(&project, settings["useVenv"].as_bool().unwrap_or(true))?;
    let output = if installer == "pip" {
        run(&python, &["-m", "ensurepip", "--upgrade"], &project)?
    } else {
        let pip_ready = if tool_available("pip", &python, &project) { true } else {
            run(&python, &["-m", "ensurepip", "--upgrade"], &project)?.status.success()
        };
        if !pip_ready { return Err("pip could not be prepared to install uv.".into()); }
        if settings["useVenv"].as_bool().unwrap_or(true) {
            run(&python, &["-m", "pip", "install", "uv"], &project)?
        } else {
            run(&python, &["-m", "pip", "install", "--user", "uv"], &project)?
        }
    };
    if !output.status.success() { return Err(output_error(&output, "The selected installer could not be installed.")); }
    Ok(json!({"ok":true,"installer":installer}))
}

/// Gera requirements.txt atomicamente a partir do ambiente Python selecionado.
pub(crate) fn generate_requirements(data_dir: &Path, body: &Value) -> Result<Value, String> {
    let settings = load_settings(data_dir);
    let project = project_path(body)?;
    let python = selected_python(&project, settings["useVenv"].as_bool().unwrap_or(true))?;
    let script = "import importlib.metadata as m; print('\\n'.join(sorted({f'{d.metadata[\"Name\"]}=={d.version}' for d in m.distributions() if d.metadata.get(\"Name\") and d.metadata[\"Name\"].lower() not in {\"pip\",\"setuptools\",\"wheel\"}}, key=str.lower)))";
    let output = run(&python, &["-c", script], &project)?;
    if !output.status.success() { return Err(output_error(&output, "Dependencies could not be inspected.")); }
    let target = project.join("requirements.txt");
    if target.symlink_metadata().is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err("requirements.txt is a symbolic link and will not be overwritten.".into());
    }
    let temporary = project.join(".requirements.txt.tscide.tmp");
    fs::write(&temporary, &output.stdout).map_err(|_| "requirements.txt could not be prepared.".to_string())?;
    let backup = project.join(".requirements.txt.tscide.backup");
    if backup.exists() { return Err("A previous requirements.txt backup must be reviewed first.".into()); }
    if target.exists() {
        fs::rename(&target, &backup).map_err(|_| "The existing requirements.txt could not be preserved.".to_string())?;
    }
    if let Err(error) = fs::rename(&temporary, &target) {
        if backup.exists() { let _ = fs::rename(&backup, &target); }
        return Err(format!("requirements.txt could not be saved: {error}"));
    }
    if backup.exists() { fs::remove_file(&backup).map_err(|_| "requirements.txt was saved, but its temporary backup could not be removed.".to_string())?; }
    Ok(json!({"ok":true,"path":target.to_string_lossy(),"useVenv":settings["useVenv"]}))
}

/// Instala requirements.txt usando pip ou uv no ambiente selecionado.
pub(crate) fn install_requirements(data_dir: &Path, body: &Value) -> Result<Value, String> {
    let settings = load_settings(data_dir);
    let project = project_path(body)?;
    if !project.join("requirements.txt").is_file() { return Err("Generate requirements.txt first.".into()); }
    let python = selected_python(&project, settings["useVenv"].as_bool().unwrap_or(true))?;
    let installer = settings["installer"].as_str().unwrap_or("pip");
    if !tool_available(installer, &python, &project) {
        return Ok(json!({"ok":false,"missingTool":installer,"requiresConfirmation":true}));
    }
    let output = if installer == "pip" {
        run(&python, &["-m", "pip", "install", "-r", "requirements.txt"], &project)?
    } else if command_works(Path::new("uv"), &["--version"]) {
        let python_arg = python.to_string_lossy().to_string();
        run(Path::new("uv"), &["pip", "install", "--python", &python_arg, "-r", "requirements.txt"], &project)?
    } else {
        let python_arg = python.to_string_lossy().to_string();
        run(&python, &["-m", "uv", "pip", "install", "--python", &python_arg, "-r", "requirements.txt"], &project)?
    };
    if !output.status.success() { return Err(output_error(&output, "Dependencies could not be installed.")); }
    Ok(json!({"ok":true,"installer":installer,"useVenv":settings["useVenv"]}))
}

/// Prepara o ambiente usado pelo terminal do projeto quando `.venv` está habilitada.
pub(crate) fn terminal_environment(data_dir: &Path, body: &Value) -> Result<Value, String> {
    let settings = load_settings(data_dir);
    if !settings["useVenv"].as_bool().unwrap_or(true) {
        return Ok(json!({"useVenv":false,"activateCommand":Value::Null}));
    }
    let project = project_path(body)?;
    selected_python(&project, true)?;
    let command = if cfg!(windows) { "& .\\.venv\\Scripts\\Activate.ps1" } else { ". ./.venv/bin/activate" };
    Ok(json!({"useVenv":true,"activateCommand":command}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_reject_unknown_installer() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(load_settings(root.path()), json!({"installer":"pip","useVenv":true}));
        save_settings(root.path(), &json!({"installer":"uv","useVenv":false})).unwrap();
        assert_eq!(load_settings(root.path()), json!({"installer":"uv","useVenv":false}));
        assert!(save_settings(root.path(), &json!({"installer":"shell","useVenv":true})).is_err());
    }

    #[test]
    fn requirements_generation_without_venv_does_not_create_environment() {
        if find_python().is_none() { return; }
        let data = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        save_settings(data.path(), &json!({"installer":"pip","useVenv":false})).unwrap();
        let result = generate_requirements(data.path(), &json!({"path":project.path()})).unwrap();
        assert_eq!(result["ok"], true);
        assert!(project.path().join("requirements.txt").is_file());
        assert!(!project.path().join(".venv").exists());
        let environment = terminal_environment(data.path(), &json!({"path":project.path()})).unwrap();
        assert_eq!(environment["useVenv"], false);
        assert!(environment["activateCommand"].is_null());
    }
}
