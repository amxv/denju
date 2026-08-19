package bundle

import (
	"archive/tar"
	"compress/gzip"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"sort"
	"testing"
)

func TestValidateSkillsSortsAndDeduplicates(t *testing.T) {
	root := t.TempDir()
	for _, name := range []string{"zeta", "alpha"} {
		if err := os.Mkdir(filepath.Join(root, name), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	got, err := ValidateSkills(root, []string{"zeta", "alpha", "zeta"})
	if err != nil {
		t.Fatal(err)
	}
	want := []string{"alpha", "zeta"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("got %v, want %v", got, want)
	}
}

func TestValidateSkillsRejectsTraversal(t *testing.T) {
	if _, err := ValidateSkills(t.TempDir(), []string{"../outside"}); err == nil {
		t.Fatal("expected invalid skill name error")
	}
}

func TestCreatePreservesCompleteSkill(t *testing.T) {
	root := t.TempDir()
	scripts := filepath.Join(root, "agentbox", "scripts")
	if err := os.MkdirAll(scripts, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "agentbox", "SKILL.md"), []byte("# Agentbox\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(scripts, "install.sh"), []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	if runtime.GOOS != "windows" {
		if err := os.Symlink("scripts/install.sh", filepath.Join(root, "agentbox", "install")); err != nil {
			t.Fatal(err)
		}
	}

	archivePath := filepath.Join(t.TempDir(), "skills.tar.gz")
	if err := Create(root, []string{"agentbox"}, archivePath); err != nil {
		t.Fatal(err)
	}
	entries := readArchive(t, archivePath)
	want := []string{"agentbox/", "agentbox/SKILL.md", "agentbox/scripts/", "agentbox/scripts/install.sh"}
	if runtime.GOOS != "windows" {
		want = append(want, "agentbox/install")
	}
	sort.Strings(want)
	if !reflect.DeepEqual(entries, want) {
		t.Fatalf("archive entries = %v, want %v", entries, want)
	}
}

func TestCreateRefusesToOverwriteArchive(t *testing.T) {
	root := t.TempDir()
	if err := os.Mkdir(filepath.Join(root, "agentbox"), 0o755); err != nil {
		t.Fatal(err)
	}
	archivePath := filepath.Join(t.TempDir(), "skills.tar.gz")
	if err := os.WriteFile(archivePath, []byte("existing"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := Create(root, []string{"agentbox"}, archivePath); err == nil {
		t.Fatal("expected existing archive error")
	}
}

func readArchive(t *testing.T, path string) []string {
	t.Helper()
	file, err := os.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	gzipReader, err := gzip.NewReader(file)
	if err != nil {
		t.Fatal(err)
	}
	defer gzipReader.Close()

	var entries []string
	reader := tar.NewReader(gzipReader)
	for {
		header, err := reader.Next()
		if err == io.EOF {
			break
		}
		if err != nil {
			t.Fatal(err)
		}
		entries = append(entries, header.Name)
	}
	sort.Strings(entries)
	return entries
}
