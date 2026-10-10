"""Real paired official XML must bind to the owning wrapper invocation (#1223)."""
import datetime
import hashlib
import importlib.util
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('cts_pm', ROOT / 'tools/cts-pm.py')
pm = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pm)
MODULE = 'CtsClassloaderSplitsHostTestCases'
PINS = {'native': 'bbd45434c2fc3ce264d6e4135b84d0a0c29fa93f1f9d505ba02e8c8a5ff7905b',
        'original': '4397b4c379cff08eb5301b656600ab3127659f316a9da5b2aa63049b7141cea1'}


def millis(hour, minute, second):
    timezone = datetime.timezone(datetime.timedelta(hours=9))
    return int(datetime.datetime(2026, 10, 9, hour, minute, second,
                                 tzinfo=timezone).timestamp() * 1000)


class OfficialInvocation(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.harness = Path(self.temp.name) / 'android-cts'
        self.data = {}
        for label, pin in PINS.items():
            fixture = ROOT / 'target/aim/cts-collector-1223-input' / (label + '.xml')
            self.assertTrue(fixture.is_file(), f'NOT RUN: missing captured official XML {fixture}')
            data = fixture.read_bytes()
            self.assertEqual(hashlib.sha256(data).hexdigest(), pin)
            self.data[label] = data
            self.stage(label, data)
        self.override = patch.object(pm, 'HARNESS', self.harness)
        self.override.start()
        self.addCleanup(self.override.stop)

    def stage(self, name, data):
        path = self.harness / 'results' / name / 'test_result.xml'
        path.parent.mkdir(parents=True)
        path.write_bytes(data)
        return path

    def scope(self, label):
        port = 5627 if label == 'native' else 5637
        args = SimpleNamespace(port=port, cts_args=[])
        # Captured owner log brackets: native15:09:47→15:24:04,
        # original15:14:11→15:22:31. These are not XML-derived selection times.
        start, finish = ((millis(15, 9, 46), millis(15, 24, 5)) if label == 'native'
                         else (millis(15, 14, 10), millis(15, 22, 32)))
        return {'command': pm.invocation_command(MODULE, args), 'serial': f'127.0.0.1:{port}',
                'started_ms': start, 'finished_ms': finish}

    def test_actual_parallel_xml_preserves_each_serial_and_full_instant_results(self):
        for label, counts in [('native', {'pass': 9}), ('original', {'pass': 4, 'fail': 5})]:
            path = pm.result_for(MODULE, set(), self.scope(label))
            self.assertEqual(path.parent.name, label)
            self.assertEqual(path.read_bytes(), self.data[label])
            result = pm.summarize(path)
            self.assertEqual(result['counts'], counts)
            self.assertEqual([m['name'] for m in result['modules']], [MODULE, MODULE + '[instant]'])
            self.assertEqual([len(m['tests']) for m in result['modules']], [5, 4])
            self.assertTrue(result['complete'])
            self.assertEqual(result['invocation']['devices'], [self.scope(label)['serial']])

    def test_same_command_duplicate_stays_ambiguous(self):
        self.stage('newer-native', self.data['native'])
        with self.assertRaisesRegex(ValueError, 'found 2'):
            pm.result_for(MODULE, set(), self.scope('native'))
        self.assertEqual(pm.result_for(MODULE, set(), self.scope('original')).parent.name, 'original')

    def test_exact_command_device_and_owner_lifetime_are_all_required(self):
        scope = self.scope('native')
        with self.assertRaisesRegex(ValueError, 'found 0'):
            pm.result_for(MODULE, set(), dict(scope, command=scope['command'] + ['--test', 'foreign-filter']))
        with self.assertRaisesRegex(ValueError, 'found 0'):
            pm.result_for(MODULE, set(), dict(scope, serial='127.0.0.1:5637'))
        with self.assertRaisesRegex(ValueError, 'found 0'):
            pm.result_for(MODULE, set(), dict(scope, finished_ms=millis(15, 24, 2)))
        with self.assertRaisesRegex(ValueError, 'found 0'):
            pm.result_for(MODULE, set(), dict(scope, started_ms=millis(15, 9, 51)))
        with self.assertRaisesRegex(ValueError, 'clock moved backwards'):
            pm.result_for(MODULE, set(), dict(scope, finished_ms=scope['started_ms'] - 1))
        before = {str(self.harness / 'results/native')}
        with self.assertRaisesRegex(ValueError, 'found 0'):
            pm.result_for(MODULE, before, scope)
        malformed = self.stage('malformed-unrelated', b'<not-complete')
        with self.assertRaisesRegex(ValueError, 'cannot bind new official XML .*malformed-unrelated'):
            pm.result_for(MODULE, set(), scope)
        # A previously existing malformed file is outside this invocation.
        self.assertEqual(pm.result_for(MODULE, {str(malformed.parent)}, scope).parent.name, 'native')
        missing = self.stage('missing-metadata', (
            '<Result suite_name="CTS" suite_version="16_r1" suite_build_number="13467367" '
            'command_line_args="' + ' '.join(scope['command']) + '" />').encode())
        before = {str(malformed.parent), str(self.harness / 'results/native')}
        with self.assertRaisesRegex(ValueError, 'found 0'):
            pm.result_for(MODULE, before, scope)
        self.assertIsNone(pm.summarize(missing)['invocation']['start_ms'])
        for label in PINS:
            self.assertEqual((self.harness / 'results' / label / 'test_result.xml').read_bytes(), self.data[label])


if __name__ == '__main__':
    unittest.main()
