#!/usr/bin/env bash
# Автоочистка кэша сборки, чтобы target/ и кэши инструментов не засоряли систему.
#
# Что делает:
#   * cargo sweep --time N  — удаляет ТОЛЬКО устаревшие артефакты в target/
#     (старые версии зависимостей, неиспользуемые профили). Текущая сборка и
#     свежие артефакты остаются нетронутыми, инкрементальная компиляция не ломается.
#   * npm cache verify      — чистит битые/просроченные записи глобального кэша npm.
#
# НЕ делает `cargo clean` — это стёрло бы весь кэш и заставило пересобирать с нуля.
#
# Обычно запускается systemd --user таймером (см. cleanup-build-cache.timer),
# но можно вызвать и вручную:
#   scripts/cleanup-build-cache.sh [DAYS]
# где DAYS — возраст артефактов для удаления (по умолчанию 7).

set -euo pipefail

# Возраст неиспользуемых артефактов (в днях), после которого их можно удалять.
DAYS="${1:-${MXKS_SWEEP_DAYS:-7}}"

# Корень проекта: каталог на уровень выше этого скрипта.
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"

log() { printf '[cleanup-build-cache] %s\n' "$*"; }

# --- Rust target/ через cargo-sweep -----------------------------------------
if command -v cargo-sweep >/dev/null 2>&1 || cargo sweep --help >/dev/null 2>&1; then
  if [[ -d "${PROJECT_ROOT}/target" ]]; then
    before="$(du -sh "${PROJECT_ROOT}/target" 2>/dev/null | cut -f1 || echo '?')"
    log "cargo sweep --time ${DAYS} (target было: ${before})"
    # --recursive обходит вложенные target/, если появятся; здесь один корень.
    cargo sweep --time "${DAYS}" "${PROJECT_ROOT}" || log "cargo sweep вернул ошибку, пропускаю"
    after="$(du -sh "${PROJECT_ROOT}/target" 2>/dev/null | cut -f1 || echo '?')"
    log "target стало: ${after}"
  else
    log "target/ отсутствует — нечего чистить"
  fi
else
  log "cargo-sweep не установлен (cargo install cargo-sweep) — Rust-кэш пропущен"
fi

# --- npm кэш ----------------------------------------------------------------
if command -v npm >/dev/null 2>&1; then
  npm_cache="$(npm config get cache 2>/dev/null || echo '')"
  if [[ -n "${npm_cache}" && -d "${npm_cache}" ]]; then
    before="$(du -sh "${npm_cache}" 2>/dev/null | cut -f1 || echo '?')"
    log "npm cache verify (было: ${before})"
    npm cache verify >/dev/null 2>&1 || log "npm cache verify вернул ошибку, пропускаю"
    after="$(du -sh "${npm_cache}" 2>/dev/null | cut -f1 || echo '?')"
    log "npm cache стало: ${after}"
  fi
else
  log "npm не найден — кэш npm пропущен"
fi

log "готово"
