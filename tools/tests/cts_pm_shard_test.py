"""Deterministic PackageManager CTS shard ownership and aggregation."""
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('cts_pm_shard', ROOT / 'tools/cts-pm-shard.py')
shard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shard)


class ShardPlanTest(unittest.TestCase):
    def test_plan_is_exact_ordered_partition(self):
        plan = shard.plan()
        names = [row['name'] for row in shard.full_manifest()]
        prefix, tail = plan['assignments']
        self.assertEqual((prefix['start_index'], prefix['end_index']), (1, 3))
        self.assertEqual((tail['start_index'], tail['end_index']), (4, len(names)))
        self.assertEqual(prefix['modules'] + tail['modules'], names)
        self.assertFalse(set(prefix['modules']) & set(tail['modules']))
        self.assertEqual(plan['manifest_sha256'], shard.digest(shard.MANIFEST))

    def test_manifest_must_retain_exactly_sixty_six_modules(self):
        with tempfile.TemporaryDirectory() as temporary:
            manifest = Path(temporary) / 'manifest.json'
            manifest.write_text(json.dumps({'modules': [
                {'name': f'CtsFixture{index}', 'kind': 'host'} for index in range(65)]}))
            with patch.object(shard, 'MANIFEST', manifest):
                with self.assertRaisesRegex(ValueError, 'exactly 66'):
                    shard.full_manifest()

    def test_tail_provenance_retains_full_manifest_and_ownership(self):
        value = shard.tail_provenance({'manifest_sha256': shard.digest(shard.MANIFEST),
                                       'modules': shard.module_names(4, len(shard.full_manifest()))})
        self.assertEqual(value['full_modules'], [row['name'] for row in shard.full_manifest()])
        self.assertEqual(value['full_manifest_sha256'], value['manifest_sha256'])
        self.assertEqual(value['shard']['modules'], value['modules'])
        self.assertEqual(value['shard']['owner'], 'tail')

    def test_selected_tail_is_indices_four_through_sixty_six_and_restores_runner(self):
        original = shard.cts_pm.modules
        with shard.selected_tail():
            self.assertEqual([row['name'] for row in shard.cts_pm.modules()],
                             shard.module_names(4, len(shard.full_manifest())))
        self.assertIs(shard.cts_pm.modules, original)


class AggregateStructureTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.addCleanup(self.temp.cleanup)
        names = [row['name'] for row in shard.full_manifest()]
        shared = {
            'label': 'native', 'manifest_sha256': shard.digest(shard.MANIFEST),
            'campaign_script_sha256': shard.digest(shard.RUNNER), 'release': '16_r1',
            'image': str(self.root / 'image'), 'image_receipt_sha256': 'fixture',
            'cts_args': [], 'linux_run': str(self.root / 'linux-run'),
            'linux_run_sha256': 'fixture', 'runtime_files_sha256': {'fixture': 'fixture'},
            'cts_host_tools': {'fixture': 'fixture'},
            'execution_tools': {'fixture': {'path': 'fixture', 'sha256': 'fixture'}},
        }
        self.prefix = self.root / 'prefix'
        self.tail = self.root / 'tail'
        for directory in (self.prefix, self.tail):
            directory.mkdir()
            (directory / 'owner.json').write_text('{}')
            (directory / 'result.xml').write_text('<fixture />')
        prefix = {**copy.deepcopy(shared), 'modules': names, 'data': str(self.root / 'prefix-data'),
                  'port': 5600, 'wrapper_bundle': {'directory': str(self.prefix / 'bundle'), 'identity': 'same'},
                  'runs': [{'module': name} for name in names[:3]]}
        tail = shard.tail_provenance({**copy.deepcopy(shared), 'modules': names[3:],
                                      'data': str(self.root / 'tail-data'), 'port': 5700,
                                      'wrapper_bundle': {'directory': str(self.tail / 'bundle'), 'identity': 'same'},
                                      'runs': [{'module': name} for name in names[3:]]})
        (self.prefix / 'campaign.json').write_text(json.dumps(prefix))
        (self.tail / 'campaign.json').write_text(json.dumps(tail))

    def fake_validate(self, directory, campaign, owned, *, tail):
        evidence = []
        for index, name in enumerate(owned):
            xml = directory / ('result-' + str(index) + '.xml')
            owner = directory / ('owner-' + str(index) + '.json')
            xml.write_text('fixture')
            owner.write_text('fixture')
            evidence.append({'module': name, 'tests': 1, 'pass': 1,
                             'xml': str(xml), 'xml_sha256': name,
                             'owner_receipt_sha256': name,
                             '_owner_receipt': str(owner),
                             '_test_identities': [(name, 'arm64-v8a', 'Fixture', 'test')]})
        return evidence

    def test_aggregate_requires_exact_union_and_isolated_resources(self):
        with patch.object(shard, '_validate_owned_campaign', side_effect=self.fake_validate), \
             patch.object(shard, '_same_generation'):
            result = shard.aggregate(self.prefix, self.tail)
        self.assertTrue(result['acceptance_pass'])
        self.assertEqual(result['counts'], {'modules': 66, 'tests': 66, 'pass': 66})
        self.assertEqual([row['module'] for row in result['module_evidence']],
                         [row['name'] for row in shard.full_manifest()])

    def test_aggregate_rejects_shared_data_or_port_pair(self):
        tail_path = self.tail / 'campaign.json'
        original = tail_path.read_text()
        prefix = json.loads((self.prefix / 'campaign.json').read_text())
        for key, value, message in (
                ('data', prefix['data'], 'DATA'),
                ('port', prefix['port'] + 1, 'port pairs')):
            with self.subTest(key=key):
                campaign = json.loads(original)
                campaign[key] = value
                tail_path.write_text(json.dumps(campaign))
                with self.assertRaisesRegex(ValueError, message):
                    shard.aggregate(self.prefix, self.tail)
        tail_path.write_text(original)

    def test_aggregate_rejects_missing_or_out_of_owner_evidence(self):
        campaign = json.loads((self.tail / 'campaign.json').read_text())
        campaign['runs'].pop()
        (self.tail / 'campaign.json').write_text(json.dumps(campaign))
        with patch.object(shard, '_validate_static_provenance'):
            with self.assertRaisesRegex(ValueError, 'missing owned modules'):
                shard._validate_owned_campaign(self.tail, campaign,
                    shard.module_names(4, len(shard.full_manifest())), tail=True)
        campaign['runs'].append({'module': shard.module_names(1, 1)[0]})
        with patch.object(shard, '_validate_static_provenance'):
            with self.assertRaisesRegex(ValueError, 'outside its assigned ownership'):
                shard._validate_owned_campaign(self.tail, campaign,
                    shard.module_names(4, len(shard.full_manifest())), tail=True)

    def test_aggregate_rejects_any_nonempty_cts_arguments(self):
        campaign = json.loads((self.tail / 'campaign.json').read_text())
        campaign['cts_args'] = ['--log-level', 'WARN']
        with self.assertRaisesRegex(ValueError, 'exactly empty'):
            shard._validate_owned_campaign(self.tail, campaign,
                shard.module_names(4, len(shard.full_manifest())), tail=True)

    def test_static_provenance_requires_native_package_registration(self):
        image = self.root / 'image'
        services = image / 'system/etc/aim/native-services'
        services.parent.mkdir(parents=True)
        (image / '.overlay-receipt').write_text('receipt')
        services.write_text('# fixture\nactivity native\n')
        campaign = json.loads((self.tail / 'campaign.json').read_text())
        campaign['image'] = str(image)
        campaign['image_receipt_sha256'] = shard.digest(image / '.overlay-receipt')
        with patch.object(shard.cts_pm, 'revalidate_execution'):
            with self.assertRaisesRegex(ValueError, 'native package service'):
                shard._validate_static_provenance(campaign)
            services.write_text('# fixture\npackage native\n')
            shard._validate_static_provenance(campaign)

    def test_aggregate_rejects_reused_xml_owner_and_test_identity(self):
        first = self.root / 'first.xml'
        second = self.root / 'second.xml'
        owner_a = self.root / 'first.owner.json'
        owner_b = self.root / 'second.owner.json'
        for path in (first, second, owner_a, owner_b):
            path.write_text('fixture')
        base = {'module': 'A', 'tests': 1, 'pass': 1, 'xml': str(first),
                '_owner_receipt': str(owner_a),
                '_test_identities': [('A', 'arm64-v8a', 'Fixture', 'test')]}
        cases = [
            ({**base, 'module': 'B', '_owner_receipt': str(owner_b)}, 'official XML'),
            ({**base, 'module': 'B', 'xml': str(second)}, 'owner receipt'),
            ({**base, 'module': 'B', 'xml': str(second), '_owner_receipt': str(owner_b)},
             'test identity'),
        ]
        for duplicate, message in cases:
            with self.subTest(message=message):
                with self.assertRaisesRegex(ValueError, message):
                    shard._reject_aggregate_duplicates([base, duplicate])


if __name__ == '__main__':
    unittest.main()
