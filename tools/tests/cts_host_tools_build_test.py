import importlib.util
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('cts_host_tools_build', ROOT / 'tools/cts-host-tools.py')
recipe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(recipe)


class CMakeBuildDirectory(unittest.TestCase):
    def test_foreign_cache_resets_only_project_build(self):
        for foreign_build, foreign_source in [(True, False), (False, True), (True, True)]:
            with self.subTest(build=foreign_build, source=foreign_source), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary).resolve()
                out = root / 'current=checkout'
                source = out / 'src/protobuf'
                build = out / 'build/protobuf'
                other = root / 'other-checkout'
                retained = [out / 'sources/protobuf.tar.gz', source / 'CMakeLists.txt',
                            out / 'build/xz/Makefile', other / 'build/protobuf/artifact']
                for path in retained:
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text('retained')
                (build / 'CMakeFiles').mkdir(parents=True)
                (build / 'CMakeFiles/stale.o').write_text('stale')
                cache_build = other / 'build/protobuf' if foreign_build else build
                cache_source = other / 'src/protobuf' if foreign_source else source
                (build / 'CMakeCache.txt').write_text(
                    f'CMAKE_CACHEFILE_DIR:INTERNAL={cache_build}\n'
                    f'CMAKE_HOME_DIRECTORY:INTERNAL={cache_source}\n')

                self.assertEqual(recipe.cmake_build_directory('protobuf', source, out), build)
                self.assertEqual(list(build.iterdir()), [])
                for path in retained:
                    self.assertEqual(path.read_text(), 'retained')

    def test_matching_cache_preserves_incremental_build(self):
        with tempfile.TemporaryDirectory() as temporary:
            out = Path(temporary).resolve() / 'current=checkout'
            source = out / 'src/protobuf'
            build = out / 'build/protobuf'
            build.mkdir(parents=True)
            cache = build / 'CMakeCache.txt'
            contents = (f'# Another path in a comment: {out / "other-checkout"}\n'
                        f'CMAKE_CACHEFILE_DIR:INTERNAL={build}\n'
                        f'// Another source: {out / "other-source"}\n'
                        f'CMAKE_HOME_DIRECTORY:INTERNAL={source}\n')
            cache.write_text(contents)
            artifact = build / 'artifact'
            artifact.write_text('incremental')

            self.assertEqual(recipe.cmake_build_directory('protobuf', source, out), build)
            self.assertEqual(cache.read_text(), contents)
            self.assertEqual(artifact.read_text(), 'incremental')

    def test_no_cache_creates_build_directory(self):
        with tempfile.TemporaryDirectory() as temporary:
            out = Path(temporary)
            build = recipe.cmake_build_directory('protobuf', out / 'src/protobuf', out)
            self.assertTrue(build.is_dir())


if __name__ == '__main__':
    unittest.main()
