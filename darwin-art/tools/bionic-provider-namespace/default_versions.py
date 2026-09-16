"""Pinned Android16 system/APEX ELF default-version metadata."""
import csv
import hashlib

# SONAME: (original ELF SHA256, TSV SHA256); source files record guest paths.
SOURCES = {
    "liblog.so": ("c49e9b479119d999fd339ccd404edd519e0295629a9b41ca97bd06d50602bea5",
                  "3f05780493185e0f9c9e0152f447d3b4d468d95e1f5b96e9f5ade4fd5b90d98d"),
    "libc.so": ("faa44ff59c5bc4cfc5413868bed9868879d2a1ad87dbb89b44121810c469dc8c",
                "d9f5e1e4ef2b12316163ae66dacb2cec7ecc2d57963cce4ca274cf607597c647"),
    "libdl.so": ("9f6447d910742a57b398a6a3f31746e02302ee9547da6d02757f001058a69df3",
                 "4e4f60b15f3fb1e835c575ce4a745ea8d798b18edec300bf4021999b870e8b70"),
    "libm.so": ("44433149f62eeb8f09f38ecd3566683fac01a0f89de6ea6d15f6e4deca1ba4bc",
                "7e444fd2766caabdfeef908efb68e1d9858c88737723fb5b6e8fdac186dfce54"),
}


def load_defaults(directory):
    result = {}
    for soname, (_, digest) in SOURCES.items():
        path = directory / (soname.removesuffix(".so") + "-default-versions.tsv")
        if hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError(f"pinned {soname} default-version metadata drift")
        with path.open(newline="") as stream:
            result[soname] = {r["symbol"]: r["default_version"]
                              for r in csv.DictReader(stream, delimiter="\t")}
    return result
