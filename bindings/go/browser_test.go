package omoikane

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestNavigateEvaluateAndContent(t *testing.T) {
	browser, err := NewBrowser()
	if err != nil {
		t.Fatal(err)
	}
	defer browser.Close()

	if err := browser.Navigate(`data:text/html,<html><body><main id="app">go</main></body></html>`); err != nil {
		t.Fatal(err)
	}

	payload, err := browser.Evaluate(`document.getElementById("app").nodeName`)
	if err != nil {
		t.Fatal(err)
	}

	var result struct {
		Result struct {
			Type  string `json:"type"`
			Value string `json:"value"`
		} `json:"result"`
	}
	if err := json.Unmarshal(payload, &result); err != nil {
		t.Fatal(err)
	}
	if result.Result.Value != "MAIN" {
		t.Fatalf("unexpected evaluate result: %s", payload)
	}

	content, err := browser.Content()
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(content, `<main id="app">go</main>`) {
		t.Fatalf("unexpected content: %s", content)
	}
}

func TestEvaluateReturnsError(t *testing.T) {
	browser, err := NewBrowser()
	if err != nil {
		t.Fatal(err)
	}
	defer browser.Close()

	if _, err := browser.Evaluate(`(()`); err == nil {
		t.Fatal("expected syntax error")
	}
}
