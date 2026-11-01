"""Run the production native parcel through the original app_process/JNI/libgui."""
import pathlib
import subprocess
import sys
import time

root, image, sdk, work = map(pathlib.Path, sys.argv[1:5])
jdk = root / '_build/java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home'
tools = root / '_build/java/build-tools-36.0.0/android-16'
def run(args, env=None):
    p = subprocess.run(list(map(str, args)), env=env, capture_output=True, text=True, timeout=60)
    assert p.returncode == 0, (p.returncode, p.stdout, p.stderr)
    return p.stdout
run([jdk/'bin/javac', '--release', '17', '-d', work, '-classpath', sdk/'platforms/android-36/android.jar',
     root/'crates/aim-services/tests/fixtures/surface/SurfaceParcelOracle.java'])
run([jdk/'bin/java', '-cp', tools/'lib/d8.jar', 'com.android.tools.r8.D8', '--min-api', '36',
     '--output', work, work/'SurfaceParcelOracle.class'])
ctl = [root/'target/release/aimctl', '--data', work/'guest']
(work/'empty').mkdir()
try:
    run(ctl + ['start', '--windows', '--image', image, '--userdata', work/'empty'])
    for _ in range(600):
        p = subprocess.run(ctl + ['shell', 'getprop sys.boot_completed'], capture_output=True, text=True, timeout=30)
        with (work/'readiness.log').open('a') as f: f.write(repr((p.returncode, p.stdout, p.stderr))+'\n')
        if p.returncode == 0 and p.stdout.strip() == '1': break
        time.sleep(.5)
    else: raise AssertionError('oracle boot did not complete')
    mapped = pathlib.Path(next(l.split('\t')[2] for l in (work/'guest.run/path-map').read_text().splitlines()
                               if len(l.split('\t')) >= 3 and l.split('\t')[1] == '/data'))
    for name in ['classes.dex', 'input', 'input.offsets']:
        (mapped/'local/tmp'/name).write_bytes((work/name).read_bytes())
    output = run(ctl + ['shell', 'CLASSPATH=/data/local/tmp/classes.dex app_process /system/bin SurfaceParcelOracle /data/local/tmp/input'])
    assert 'ok original native reparent parcel' in output, output
    print(output, end='')
finally:
    try: (work/'oracle-logcat.txt').write_bytes(subprocess.check_output(list(map(str, ctl + ['shell', 'logcat -d -b all -v threadtime'])), timeout=60))
    finally:
        run(ctl + ['stop'])
        assert str(work/'guest.asif') not in run(['hdiutil', 'info'])
        (work/'guest.asif').unlink(missing_ok=True)
