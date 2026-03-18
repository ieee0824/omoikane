import ctypes
from pathlib import Path


lib_path = Path(__file__).resolve().parents[2] / "target" / "debug" / "libomoikane.dylib"
lib = ctypes.CDLL(str(lib_path))

lib.omoikane_init.restype = ctypes.c_void_p
lib.omoikane_navigate.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
lib.omoikane_navigate.restype = ctypes.c_bool
lib.omoikane_evaluate.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
lib.omoikane_evaluate.restype = ctypes.c_void_p
lib.omoikane_string_free.argtypes = [ctypes.c_void_p]
lib.omoikane_free.argtypes = [ctypes.c_void_p]

browser = lib.omoikane_init()
assert browser
assert lib.omoikane_navigate(
    browser, b"data:text/html,<html><body><main id='app'>python</main></body></html>"
)

result_ptr = lib.omoikane_evaluate(browser, b"document.getElementById('app').nodeName")
result = ctypes.c_char_p(result_ptr).value.decode("utf-8")
print(result)
lib.omoikane_string_free(result_ptr)
lib.omoikane_free(browser)
