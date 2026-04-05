"""Basic tests for flutmax Python bindings."""
import flutmax_py
import json

def test_compile():
    source = "wire osc = cycle~(440);\nout audio: signal = osc;\n"
    result = flutmax_py.compile(source)
    data = json.loads(result)
    assert "patcher" in data

def test_compile_error():
    try:
        flutmax_py.compile("wire osc = ;")
        assert False, "should raise"
    except ValueError:
        pass

def test_parse():
    source = "in freq: float;\nwire osc = cycle~(freq);\nout audio: signal = osc;\n"
    result = flutmax_py.parse(source)
    data = json.loads(result)
    assert "wires" in data
    assert "in_decls" in data

def test_version():
    assert flutmax_py.__version__ == "0.1.0"

if __name__ == "__main__":
    test_compile()
    test_compile_error()
    test_parse()
    test_version()
    print("All Python tests passed!")
