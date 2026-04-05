"""Tests for flutmax Python bindings."""
import flutmax_py
import json
import os
import tempfile

# --- String API ---

def test_compile():
    source = "wire osc = cycle~(440);\nout audio: signal = osc;\n"
    result = flutmax_py.compile(source)
    data = json.loads(result)
    assert "patcher" in data
    assert any("cycle~" in b["box"].get("text", "") for b in data["patcher"]["boxes"])

def test_compile_error():
    try:
        flutmax_py.compile("wire osc = ;")
        assert False, "should raise"
    except ValueError:
        pass

def test_decompile():
    source = "wire osc = cycle~(440);\nout audio: signal = osc;\n"
    maxpat = flutmax_py.compile(source)
    result = flutmax_py.decompile(maxpat)
    assert "cycle~" in result

def test_parse():
    source = "in freq: float;\nwire osc = cycle~(freq);\nout audio: signal = osc;\n"
    result = flutmax_py.parse(source)
    data = json.loads(result)
    assert "wires" in data
    assert "in_decls" in data

def test_roundtrip():
    source = "wire osc = cycle~(440);\nout audio: signal = osc;\n"
    maxpat = flutmax_py.compile(source)
    decompiled = flutmax_py.decompile(maxpat)
    maxpat2 = flutmax_py.compile(decompiled)
    # Both should produce valid .maxpat JSON
    data1 = json.loads(maxpat)
    data2 = json.loads(maxpat2)
    assert len(data1["patcher"]["boxes"]) == len(data2["patcher"]["boxes"])

# --- File API ---

def test_compile_file():
    with tempfile.TemporaryDirectory() as tmpdir:
        src = os.path.join(tmpdir, "test.flutmax")
        dst = os.path.join(tmpdir, "test.maxpat")
        with open(src, "w") as f:
            f.write("wire osc = cycle~(440);\nout audio: signal = osc;\n")
        flutmax_py.compile_file(src, dst)
        assert os.path.exists(dst)
        with open(dst) as f:
            data = json.load(f)
        assert "patcher" in data

def test_decompile_file():
    with tempfile.TemporaryDirectory() as tmpdir:
        # First compile to get a valid .maxpat
        src = os.path.join(tmpdir, "test.flutmax")
        maxpat = os.path.join(tmpdir, "test.maxpat")
        output = os.path.join(tmpdir, "output.flutmax")
        with open(src, "w") as f:
            f.write("wire osc = cycle~(440);\nout audio: signal = osc;\n")
        flutmax_py.compile_file(src, maxpat)
        # Then decompile
        flutmax_py.decompile_file(maxpat, output)
        assert os.path.exists(output)
        with open(output) as f:
            content = f.read()
        assert "cycle~" in content

def test_compile_file_not_found():
    try:
        flutmax_py.compile_file("/nonexistent/path.flutmax", "/tmp/out.maxpat")
        assert False, "should raise"
    except OSError:
        pass

def test_decompile_file_creates_parent_dirs():
    with tempfile.TemporaryDirectory() as tmpdir:
        src = os.path.join(tmpdir, "test.flutmax")
        maxpat = os.path.join(tmpdir, "test.maxpat")
        output = os.path.join(tmpdir, "sub", "dir", "output.flutmax")
        with open(src, "w") as f:
            f.write("wire osc = cycle~(440);\nout audio: signal = osc;\n")
        flutmax_py.compile_file(src, maxpat)
        flutmax_py.decompile_file(maxpat, output)
        assert os.path.exists(output)

# --- Metadata ---

def test_version():
    assert flutmax_py.__version__ == "0.1.1"

if __name__ == "__main__":
    test_compile()
    test_compile_error()
    test_decompile()
    test_parse()
    test_roundtrip()
    test_compile_file()
    test_decompile_file()
    test_compile_file_not_found()
    test_decompile_file_creates_parent_dirs()
    test_version()
    print("All Python tests passed!")
