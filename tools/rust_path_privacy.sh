#!/usr/bin/env bash
# Source after setting CARGO_HOME and RUSTUP_HOME. Keep source locations useful
# without recording the build machine's home or checkout in shipped binaries.
task_configure_rust_path_privacy() {
  local task_repo_root="$1" task_encoded_flags="" task_flag task_source task_public
  local task_physical task_remaining task_mapping task_keep task_retained_count=0
  local -a task_words=() task_mapping_flags=() task_retained_flags=()
  if [[ "${CARGO_ENCODED_RUSTFLAGS+x}" == x ]]; then
    # Cargo gives this variable precedence over RUSTFLAGS; preserve it verbatim.
    task_encoded_flags="$CARGO_ENCODED_RUSTFLAGS"
  elif [[ -n "${RUSTFLAGS:-}" ]]; then
    # RUSTFLAGS uses whitespace splitting, while encoded flags can retain spaces
    # inside each path mapping. Do not evaluate flags as shell code.
    while IFS=$' \t\r\v\f' read -r -a task_words; do
      [[ -n "${task_words[*]:-}" ]] || continue
      for task_flag in "${task_words[@]}"; do
        [[ -z "$task_encoded_flags" ]] || task_encoded_flags+=$'\x1f'
        task_encoded_flags+="$task_flag"
      done
    done <<< "$RUSTFLAGS"
  fi
  # Put more specific roots after the home/checkout roots: rustc applies the
  # last matching remapping. Cover symlinks as well as the supplied root paths.
  while IFS=$'\t' read -r task_source task_public; do
    [[ -n "$task_source" && "$task_source" != / ]] || continue
    task_source="${task_source%/}"
    task_mapping_flags+=("--remap-path-prefix=$task_source=$task_public")
    if [[ -d "$task_source" ]]; then
      task_physical="$(cd "$task_source" && pwd -P)"
      if [[ "$task_physical" != "$task_source" ]]; then
        task_mapping_flags+=("--remap-path-prefix=$task_physical=$task_public")
      fi
    fi
  done <<EOF
${HOME:-}	/home/build
$task_repo_root	/src
${CARGO_HOME:-}	/cargo
${RUSTUP_HOME:-}	/rustup
EOF
  # Re-sourcing env.sh should not grow the flags or trigger another full build.
  # Keep user options in order and place each required mapping at the end once.
  task_remaining="$task_encoded_flags"
  while [[ -n "$task_remaining" ]]; do
    if [[ "$task_remaining" == *$'\x1f'* ]]; then
      task_flag="${task_remaining%%$'\x1f'*}"
      task_remaining="${task_remaining#*$'\x1f'}"
    else
      task_flag="$task_remaining"
      task_remaining=""
    fi
    task_keep=1
    for task_mapping in "${task_mapping_flags[@]}"; do
      if [[ "$task_flag" == "$task_mapping" ]]; then task_keep=0; break; fi
    done
    if [[ "$task_keep" == 1 ]]; then
      task_retained_flags+=("$task_flag")
      task_retained_count=$((task_retained_count + 1))
    fi
  done
  # Direct rustc (the baseline probe) does not read Cargo's environment flags.
  if [[ "$task_retained_count" == 0 ]]; then
    HARMONY_RUSTC_FLAGS=("${task_mapping_flags[@]}")
  else
    HARMONY_RUSTC_FLAGS=("${task_retained_flags[@]}" "${task_mapping_flags[@]}")
  fi
  task_encoded_flags=""
  for task_flag in "${HARMONY_RUSTC_FLAGS[@]}"; do
    [[ -z "$task_encoded_flags" ]] || task_encoded_flags+=$'\x1f'
    task_encoded_flags+="$task_flag"
  done
  export CARGO_ENCODED_RUSTFLAGS="$task_encoded_flags"
}
