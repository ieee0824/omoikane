package omoikane

/*
#cgo CFLAGS: -I${SRCDIR}/../../include
#cgo darwin LDFLAGS: -L${SRCDIR}/../../target/debug -lomoikane
#cgo linux LDFLAGS: -L${SRCDIR}/../../target/debug -lomoikane -ldl -lm -lpthread
#include <stdlib.h>
#include "omoikane.h"
*/
import "C"

import (
	"encoding/json"
	"errors"
	"fmt"
	"runtime"
	"unsafe"
)

type Browser struct {
	ptr *C.struct_OmoikaneBrowser
}

func NewBrowser() (*Browser, error) {
	ptr := C.omoikane_init()
	if ptr == nil {
		return nil, errors.New("omoikane_init returned nil")
	}

	browser := &Browser{ptr: ptr}
	runtime.SetFinalizer(browser, (*Browser).Close)
	return browser, nil
}

func (b *Browser) Navigate(url string) error {
	if err := b.ensureOpen(); err != nil {
		return err
	}

	curl := C.CString(url)
	defer C.free(unsafe.Pointer(curl))

	if !C.omoikane_navigate(b.ptr, curl) {
		return b.lastError()
	}
	return nil
}

func (b *Browser) Evaluate(expression string) (json.RawMessage, error) {
	if err := b.ensureOpen(); err != nil {
		return nil, err
	}

	cexpr := C.CString(expression)
	defer C.free(unsafe.Pointer(cexpr))

	result := C.omoikane_evaluate(b.ptr, cexpr)
	if result == nil {
		return nil, b.lastError()
	}
	defer C.omoikane_string_free(result)

	payload := C.GoString(result)
	return json.RawMessage(payload), nil
}

func (b *Browser) Content() (string, error) {
	if err := b.ensureOpen(); err != nil {
		return "", err
	}

	result := C.omoikane_get_content(b.ptr)
	if result == nil {
		return "", b.lastError()
	}
	defer C.omoikane_string_free(result)

	return C.GoString(result), nil
}

func (b *Browser) Close() {
	if b == nil || b.ptr == nil {
		return
	}
	C.omoikane_free(b.ptr)
	b.ptr = nil
}

func (b *Browser) ensureOpen() error {
	if b == nil || b.ptr == nil {
		return errors.New("omoikane browser is closed")
	}
	return nil
}

func (b *Browser) lastError() error {
	if b == nil || b.ptr == nil {
		return errors.New("omoikane browser is closed")
	}

	ptr := C.omoikane_last_error(b.ptr)
	if ptr == nil {
		return errors.New("omoikane returned an unknown error")
	}
	defer C.omoikane_string_free(ptr)

	message := C.GoString(ptr)
	return fmt.Errorf("omoikane: %s", message)
}
