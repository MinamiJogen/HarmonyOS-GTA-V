#!/usr/bin/env python3
"""Produce a conservative, machine-readable stop/no-promotion decision."""
import argparse
import hashlib
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent


def evaluate(output):
    gates = json.loads((HERE / 'gates.json').read_text())
    run = json.loads((output / 'host-run.json').read_text())
    prepared_path = output / 'prepared-source.json'
    if hashlib.sha256(prepared_path.read_bytes()).hexdigest() != run['preparedSource']['sha256']:
        raise ValueError('prepared-source inventory changed after the measured run')
    prepared = json.loads(prepared_path.read_text())
    gate_inputs = [row for row in prepared['experimentInputs'] if Path(row['path']).name == 'gates.json']
    if len(gate_inputs) != 1 or gate_inputs[0]['sha256'] != hashlib.sha256((HERE / 'gates.json').read_bytes()).hexdigest():
        raise ValueError('decision gates differ from the frozen pre-measurement input')
    log_path = Path(run['log']['path'])
    if hashlib.sha256(log_path.read_bytes()).hexdigest() != run['log']['sha256']:
        raise ValueError('measured log changed after the recorded run')
    log = log_path.read_text()
    rows = [line.split('RENDER_SUBMIT_RESULT=', 1)[1] for line in log.splitlines()
            if 'RENDER_SUBMIT_RESULT=' in line]
    if len(rows) != 1:
        raise ValueError('expected exactly one completed real-core result')
    result = json.loads(rows[0])
    (output / 'measurements.json').write_text(json.dumps(result, indent=2) + '\n')
    correctness = (run['status'] == 'pass' and run['originalVendorInputsUnchanged']
                   and all(r['exact_match'] for r in result['differential'])
                   and result['lifecycle_modes_passed'] == 2)
    if '--release' not in run['command']:
        raise ValueError('performance decision requires release compilation')
    primary, fallback = [], []
    for row in result['benchmarks']:
        if (row['fixture'] in gates['primaryFixtures'] and
                row['draws_per_pass'] in gates['primaryDrawsPerPass']):
            change = (row['candidate_median_ns'] / row['legacy_median_ns'] - 1) * 100
            p95 = (row['candidate_p95_ns'] / row['legacy_p95_ns'] - 1) * 100
            primary.append(dict(fixture=row['fixture'], drawsPerPass=row['draws_per_pass'],
                                medianChangePercent=change, p95ChangePercent=p95,
                                passed=change <= -gates['requiredMedianImprovementPercent']
                                and p95 <= gates['maximumP95RegressionPercent']
                                and row['paired_median_delta_ns'] < 0))
        elif row['fixture'] == 'MixedFallback':
            change = (row['candidate_median_ns'] / row['legacy_median_ns'] - 1) * 100
            fallback.append(dict(fixture=row['fixture'], medianChangePercent=change,
                                 passed=change <= gates['maximumFallbackMedianRegressionPercent']))
    expected = {(fixture, draws) for fixture in gates['primaryFixtures']
                for draws in gates['primaryDrawsPerPass']}
    actual = {(row['fixture'], row['drawsPerPass']) for row in primary}
    cost_pass = (len(primary) == len(expected) and actual == expected and len(fallback) == 1
                 and all(r['passed'] for r in primary + fallback))
    # Synthetic fixtures cannot supply representative game coverage. Never
    # convert fixture eligibility into a percentage of real frames or passes.
    decision = {
        'schemaVersion': 1,
        'decision': ('STOP_CORRECTNESS' if not correctness else
                     'STOP_THIS_PROTOTYPE' if not cost_pass else 'NO_PROMOTION_COVERAGE_UNKNOWN'),
        'correctnessPassed': correctness,
        'completeHostCostGatePassed': cost_pass,
        'primary': primary, 'mixedFallback': fallback,
        'representativePassCoveragePercent': None,
        'representativeDirectDrawCoveragePercent': None,
        'realDeviceTotalFrameTimeMeasured': False,
        'successfulOptimization': False, 'productionIntegration': False,
        'gateSourceSha256': hashlib.sha256((HERE / 'gates.json').read_bytes()).hexdigest(),
        'limits': gates['limits'],
    }
    (output / 'decision.json').write_text(json.dumps(decision, indent=2) + '\n')
    print(json.dumps(decision, indent=2))
    return decision


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    evaluate(parser.parse_args().output.resolve())
