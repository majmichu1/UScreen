"""Decode the generated DTD using the EDID byte layout, independent of encoding."""
from pathlib import Path
import runpy

make_edid = runpy.run_path(str(Path(__file__).with_name("gen-edid.py")))["make_edid"]
for width, height in [(1536, 960), (2560, 1600), (2960, 1848)]:
    edid = make_edid(width, height)
    physical_width = edid[66] | ((edid[68] >> 4) << 8)
    physical_height = edid[67] | ((edid[68] & 15) << 8)
    assert (physical_width, physical_height) == (310, 194), (physical_width, physical_height)
    assert len(edid) == 128 and sum(edid) % 256 == 0
print("PASS: Python EDID physical size and checksum")
