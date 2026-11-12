#!/usr/bin/env python3

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / 'benchmarks/artifact-size/fixtures'
KB_NAMES = ['braille', 'led', 'led-dim', 'passthrough', 'shift', 'toucan-keymap']


def command(arguments, cwd=ROOT):
    result = subprocess.run(arguments, cwd=cwd, capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(f'{arguments!r}\n{result.stdout}{result.stderr}')
    return result


def digest(data):
    return hashlib.sha256(data).hexdigest()


def unsigned_leb(data, offset):
    value = shift = 0
    while True:
        byte = data[offset]
        offset += 1
        value |= (byte & 127) << shift
        if byte < 128:
            return value, offset
        shift += 7


def sections(data):
    offset = 8
    while offset < len(data):
        section_id = data[offset]
        length, start = unsigned_leb(data, offset + 1)
        end = start + length
        yield section_id, data[start:end], end - offset
        offset = end
    if offset != len(data):
        raise RuntimeError('invalid section length')


def breakdown(data):
    counts = dict(core_modules=0, core_bytes=0, code_section_bytes=0,
                  data_section_bytes=0, custom_section_bytes=0)

    def visit(module):
        core = module[:8] == b'\0asm\x01\0\0\0'
        if core:
            counts['core_modules'] += 1
            counts['core_bytes'] += len(module)
        for section_id, payload, encoded_size in sections(module):
            if section_id == 0:
                counts['custom_section_bytes'] += encoded_size
            if core and section_id == 10:
                counts['code_section_bytes'] += encoded_size
            if core and section_id == 11:
                counts['data_section_bytes'] += encoded_size
            if not core and section_id in (1, 4):
                visit(payload)

    visit(data)
    return counts


def copy_file(source, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)


def prepare_case(name, directory, kb):
    source = directory / 'src/main.wlt'
    if name in ('minimal', 'wasi-hello', 'collections'):
        copy_file(FIXTURES / f'{name}.wlt', source)
    else:
        copy_file(kb / f'components/{name}/{name}.wlt', source)
        copy_file(kb / 'wit/firmware.wit', directory / 'wit/deps/firmware.wit')
    if name == 'wasi-hello':
        copy_file(FIXTURES / 'wit/wasi-cli.wit', directory / 'wit/deps/cli.wit')
        copy_file(ROOT / 'examples/http/wit/deps/wasi-io-0.2.0/package.wit',
                  directory / 'wit/deps/io.wit')
    sources = [source]
    if name == 'led-dim':
        leaf = directory / 'src/led.wlt'
        copy_file(kb / 'components/led/led.wlt', leaf)
        sources.append(leaf)
    return sources


def input_hashes(directory):
    return {str(path.relative_to(directory)): digest(path.read_bytes())
            for path in sorted(directory.rglob('*')) if path.is_file()}


def smoke_cases(name):
    led_input = 'step({phase: 0, speed: 0, last-change-ms: 0, button-was-down: false}, 0, false)'
    led_state = '{state: {phase: 0, speed: 0, last-change-ms: 0, button-was-down: false}, red: '
    event = '{position: 0, state: pressed, timestamp-ms: 10}'
    keyboard_state = ', '.join(['43'] + ['0'] * 41)
    return {
        'minimal': [('run()', '2')],
        'wasi-hello': [(None, 'Hello, world!')],
        'collections': [('run([1,2,3,4])', '"1,4,9,16"'), ('run([])', '""')],
        'braille': [('map-cell(46)', '542'), ('map-cell(16)', '564')],
        'led': [(led_input, led_state + '255, green: 0, blue: 0}')],
        'led-dim': [(led_input, led_state + '63, green: 0, blue: 0}')],
        'passthrough': [(f'process([1], {{events: [{event}], now-ms: 10}})',
                         f'{{state: [1], events: [{event}], actions: []}}')],
        'shift': [('transform-output([], [press({usage: 4, modifiers: 0})])',
                   '([], [press({usage: 4, modifiers: 2})])')],
        'toucan-keymap': [(f'process([], {{events: [{event}], now-ms: 10}})',
                           f'{{state: [{keyboard_state}], events: [], actions: [press({{usage: 43, modifiers: 0}})]}}')],
    }[name]


def measure_case(name, directory, args):
    sources = prepare_case(name, directory, args.kb)
    inputs = input_hashes(directory)
    output = directory / 'out'
    built = command([str(args.wavelet), 'build', *map(str, sources), '-o', str(output), *args.build_arg])
    emitted = sorted(output.glob('*.wasm'))
    if name == 'led-dim':
        artifact = output / 'app.wasm'
        command([str(args.wavelet), 'compose', str(output / 'kb-led-dim.wasm'),
                 str(output / 'kb-led-demo.wasm'), '-o', str(artifact), *args.compose_arg])
    else:
        if len(emitted) != 1:
            raise RuntimeError(f'{name}: expected one component, found {emitted}')
        artifact = emitted[0]
    command(['wasm-tools', 'validate', str(artifact)])
    checks = []
    for invocation, expected in smoke_cases(name):
        cli = ['wasmtime', 'run']
        if invocation is not None:
            cli += ['--invoke', invocation]
        result = command([*cli, str(artifact)])
        if result.stdout != expected + '\n':
            raise RuntimeError(f'{name}: {invocation}: expected {expected!r}, got {result.stdout!r}')
        checks.append(dict(invocation=invocation, stdout=result.stdout, exit_status=0))
    data = artifact.read_bytes()
    interface = command(['wasm-tools', 'component', 'wit', str(artifact)]).stdout
    return dict(deployed_bytes=len(data), artifact_sha256=digest(data),
                interface_sha256=digest(interface.encode()), inputs=inputs,
                emitted_units_bytes=sum(path.stat().st_size for path in emitted if path.name != 'app.wasm'),
                sections=breakdown(data), checks=checks,
                build_warnings=built.stderr.strip())


def run(args):
    cases = {}
    with tempfile.TemporaryDirectory(prefix='wavelet-artifact-size-') as temporary:
        for name in ['minimal', 'wasi-hello', 'collections', *KB_NAMES]:
            cases[name] = measure_case(name, Path(temporary) / name, args)
            print(f'{name}: {cases[name]["deployed_bytes"]:,} bytes', flush=True)
    if args.baseline:
        previous = json.loads(args.baseline.read_text())
        for name, case in cases.items():
            baseline = previous['artifacts'][name]
            for field in ['inputs', 'interface_sha256', 'checks']:
                if case[field] != baseline[field]:
                    raise RuntimeError(f'{name}: {field} differs from baseline')
    return dict(stage=args.stage, compiler_commit=args.compiler_commit or command(['git', 'rev-parse', 'HEAD']).stdout.strip(),
                compiler_sha256=digest(args.wavelet.read_bytes()), build_arguments=args.build_arg,
                compose_arguments=args.compose_arg,
                tools={tool: command([tool, '--version']).stdout.strip()
                       for tool in ['wasm-tools', 'wasm-opt', 'wasmtime']},
                kb_commit=command(['git', 'rev-parse', 'HEAD'], cwd=args.kb).stdout.strip(),
                kb_worktree=command(['git', 'status', '--porcelain'], cwd=args.kb).stdout,
                benchmark_bytes={**{name: cases[name]['deployed_bytes']
                                    for name in ['minimal', 'wasi-hello', 'collections']},
                                 'kb-project': sum(cases[name]['deployed_bytes'] for name in KB_NAMES)},
                artifacts=cases)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--wavelet', type=Path, default=ROOT / 'target/release/wavelet')
    parser.add_argument('--kb', type=Path, required=True)
    parser.add_argument('--stage', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--baseline', type=Path)
    parser.add_argument('--compiler-commit')
    parser.add_argument('--build-arg', action='append', default=[],
                        help='Forward a build argument; use --build-arg=--no-optimize for unoptimised comparisons')
    parser.add_argument('--compose-arg', action='append', default=[])
    args = parser.parse_args()
    args.wavelet = args.wavelet.resolve()
    args.kb = args.kb.resolve()
    report = run(args)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
