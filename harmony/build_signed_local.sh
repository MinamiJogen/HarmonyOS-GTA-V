#!/bin/zsh
set -eu

task_project_dir="${0:A:h}"
task_stage_root="${HARMONY_BUILD_STAGE:-/private/tmp/harmony-runtime-probe-${UID}}"
task_stage_profile="$task_stage_root/harmony/build-profile.json5"
if [[ ! -f "$task_stage_profile" ]]; then
  print -u2 'Open the ASCII build mirror in DevEco and configure automatic device signing first.'
  exit 2
fi
python3 - "$task_stage_profile" <<'PYCONFIG'
import json
import sys
from pathlib import Path
try:
    profile = json.loads(Path(sys.argv[1]).read_text())
    app = profile.get('app', {})
    names = {item.get('name') for item in app.get('signingConfigs', []) if item.get('material')}
    if not any(item.get('signingConfig') in names for item in app.get('products', [])):
        raise ValueError('No product references an available signing material.')
except Exception:
    print('No readable local signing profile. Configure automatic signing in DevEco first.', file=sys.stderr)
    sys.exit(2)
PYCONFIG
export HARMONY_ENABLE_SIGNING=true
exec "$task_project_dir/build_unsigned.sh" "$@"
