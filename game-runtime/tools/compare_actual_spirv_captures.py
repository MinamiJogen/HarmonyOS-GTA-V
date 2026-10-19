#!/usr/bin/env python3
"""Compare exact driver-uploaded words, allowing differing executed shader coverage."""
import argparse
from collections import defaultdict
import json
from pathlib import Path


def read_capture(report):
    validation = json.loads(report.read_text())
    assert validation['result'] == 'pass'
    directory = Path(validation['session'])
    records = [json.loads(line) for line in (directory / 'manifest.jsonl').read_text().splitlines()]
    return directory, records


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('baseline_validation', type=Path)
    parser.add_argument('candidate_validation', type=Path)
    parser.add_argument('--report', required=True, type=Path)
    args = parser.parse_args()
    baseline_dir, baseline = read_capture(args.baseline_validation)
    candidate_dir, candidate = read_capture(args.candidate_validation)
    baseline_hashes = {r['sha256']: r for r in baseline}
    candidate_hashes = {r['sha256']: r for r in candidate}
    common_hashes = sorted(baseline_hashes.keys() & candidate_hashes.keys())
    # Validation already checked each hash, but compare bytes directly too.
    common = []
    for sha in common_hashes:
        left, right = baseline_hashes[sha], candidate_hashes[sha]
        identical = ((baseline_dir / left['file']).read_bytes() == (candidate_dir / right['file']).read_bytes())
        common.append({'sha256': sha, 'baselineFile': left['file'], 'candidateFile': right['file'], 'byteIdentical': identical})
    groups = []
    for records in [baseline, candidate]:
        group = defaultdict(list)
        for row in records:
            group[(row['stage'], row['entryPoint'], row['pipelineLabel'])].append(row)
        groups.append(group)
    label_comparisons = []
    ambiguous = []
    for key in sorted(groups[0].keys() & groups[1].keys(), key=str):
        left, right = groups[0][key], groups[1][key]
        if len(left) != 1 or len(right) != 1:
            ambiguous.append({'key': key, 'baselineCount': len(left), 'candidateCount': len(right)})
            continue
        label_comparisons.append({'stage': key[0], 'entryPoint': key[1], 'pipelineLabel': key[2],
                                  'baselineSHA256': left[0]['sha256'], 'candidateSHA256': right[0]['sha256'],
                                  'identical': left[0]['sha256'] == right[0]['sha256']})
    report = {
        'scope': 'Exact final device SPIR-V words, with complete validated capture ledgers. Different runs can execute different shader variants and first-use pipeline labels. Common hashes are compared byte-for-byte. A unique label is not a source/override identity proof; mismatches require separate analysis.',
        'baselineDirectory': str(baseline_dir), 'candidateDirectory': str(candidate_dir),
        'baselineUniqueModules': len(baseline), 'candidateUniqueModules': len(candidate),
        'commonExactModules': len(common), 'allCommonWordsByteIdentical': all(r['byteIdentical'] for r in common),
        'uniqueCommonLabelMatches': len(label_comparisons),
        'uniqueCommonLabelIdentical': sum(r['identical'] for r in label_comparisons),
        'uniqueCommonLabelMismatches': [r for r in label_comparisons if not r['identical']],
        'ambiguousCommonLabels': ambiguous,
        'baselineOnlyModules': [r for r in baseline if r['sha256'] not in candidate_hashes],
        'candidateOnlyModules': [r for r in candidate if r['sha256'] not in baseline_hashes],
        'commonModules': common, 'labelComparisons': label_comparisons,
    }
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({k: report[k] for k in ['baselineUniqueModules', 'candidateUniqueModules', 'commonExactModules', 'allCommonWordsByteIdentical', 'uniqueCommonLabelMatches', 'uniqueCommonLabelIdentical', 'uniqueCommonLabelMismatches']}, indent=2))


if __name__ == '__main__':
    main()
