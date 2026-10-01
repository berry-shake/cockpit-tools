use crate::modules::omp::{self, OmpAccountState};

#[tauri::command]
pub async fn omp_get_state() -> Result<OmpAccountState, String> {
    tauri::async_runtime::spawn_blocking(omp::account_state)
        .await
        .map_err(|_| "OMP 状态读取任务失败")?
}

#[tauri::command]
pub async fn omp_account_action(
    id: i64,
    expected_identity: String,
    action: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || omp::account_action(id, expected_identity, action))
        .await
        .map_err(|_| "OMP 账号操作任务失败")?
}

#[tauri::command]
pub async fn omp_login(executable: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || omp::login(executable))
        .await
        .map_err(|_| "OMP 启动任务失败")?
}
