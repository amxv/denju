package app

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestRunRootHelp(t *testing.T) {
	var out bytes.Buffer
	var errOut bytes.Buffer
	if err := Run([]string{"--help"}, &out, &errOut); err != nil {
		t.Fatalf("Run returned error: %v", err)
	}
	for _, expected := range []string{"Usage:", "--team", "--package-only"} {
		if !strings.Contains(out.String(), expected) {
			t.Fatalf("expected %q in help output: %q", expected, out.String())
		}
	}
}

func TestRunVersion(t *testing.T) {
	var out bytes.Buffer
	if err := Run([]string{"--version"}, &out, &bytes.Buffer{}); err != nil {
		t.Fatalf("Run returned error: %v", err)
	}
	if !strings.HasPrefix(out.String(), commandName+" ") {
		t.Fatalf("unexpected version output: %q", out.String())
	}
}

func TestRunPackageOnly(t *testing.T) {
	root := t.TempDir()
	skillDir := filepath.Join(root, "agentbox", "scripts")
	if err := os.MkdirAll(skillDir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "agentbox", "SKILL.md"), []byte("# Agentbox\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(skillDir, "install.sh"), []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}

	archivePath := filepath.Join(t.TempDir(), "skills.tar.gz")
	var out bytes.Buffer
	err := run(context.Background(), []string{
		"--package-only",
		"--skills-dir", root,
		"--archive", archivePath,
		"agentbox",
	}, &out, &bytes.Buffer{}, nil)
	if err != nil {
		t.Fatalf("run returned error: %v", err)
	}
	if strings.TrimSpace(out.String()) != archivePath {
		t.Fatalf("unexpected output: %q", out.String())
	}
	if info, err := os.Stat(archivePath); err != nil || info.Size() == 0 {
		t.Fatalf("archive was not created: info=%v err=%v", info, err)
	}
}

func TestRunRequiresTeamWhenSharing(t *testing.T) {
	err := run(context.Background(), []string{"agentbox"}, &bytes.Buffer{}, &bytes.Buffer{}, nil)
	if err == nil || !strings.Contains(err.Error(), "--team is required") {
		t.Fatalf("expected team error, got %v", err)
	}
}

func TestBundleMessageUsesSelectedTeam(t *testing.T) {
	message := bundleMessage([]string{"agentbox", "dogfood"}, "engineering")
	for _, expected := range []string{"`engineering`", "`agentbox`", "`dogfood`"} {
		if !strings.Contains(message, expected) {
			t.Fatalf("expected %q in message: %q", expected, message)
		}
	}
}
