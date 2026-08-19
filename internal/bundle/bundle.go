package bundle

import (
	"archive/tar"
	"compress/gzip"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
)

func ValidateSkills(root string, names []string) ([]string, error) {
	seen := make(map[string]bool, len(names))
	clean := make([]string, 0, len(names))
	for _, name := range names {
		if name == "" || name == "." || name == ".." || filepath.Base(name) != name {
			return nil, fmt.Errorf("invalid skill name %q", name)
		}
		if seen[name] {
			continue
		}
		info, err := os.Stat(filepath.Join(root, name))
		if err != nil {
			return nil, fmt.Errorf("skill %q: %w", name, err)
		}
		if !info.IsDir() {
			return nil, fmt.Errorf("skill %q is not a directory", name)
		}
		seen[name] = true
		clean = append(clean, name)
	}
	sort.Strings(clean)
	return clean, nil
}

func Create(root string, names []string, destination string) (returnErr error) {
	file, err := os.OpenFile(destination, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o644)
	if err != nil {
		return err
	}
	defer func() {
		if err := file.Close(); returnErr == nil && err != nil {
			returnErr = err
		}
	}()

	gzipWriter := gzip.NewWriter(file)
	defer func() {
		if err := gzipWriter.Close(); returnErr == nil && err != nil {
			returnErr = err
		}
	}()
	tarWriter := tar.NewWriter(gzipWriter)
	defer func() {
		if err := tarWriter.Close(); returnErr == nil && err != nil {
			returnErr = err
		}
	}()

	for _, name := range names {
		base, err := filepath.EvalSymlinks(filepath.Join(root, name))
		if err != nil {
			return fmt.Errorf("resolve skill %q: %w", name, err)
		}
		if err := addSkill(tarWriter, name, base); err != nil {
			return fmt.Errorf("archive skill %q: %w", name, err)
		}
	}
	return nil
}

func addSkill(writer *tar.Writer, name, base string) error {
	return filepath.WalkDir(base, func(path string, entry fs.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		info, err := entry.Info()
		if err != nil {
			return err
		}

		relative, err := filepath.Rel(base, path)
		if err != nil {
			return err
		}
		archiveName := name
		if relative != "." {
			archiveName = filepath.Join(name, relative)
		}

		linkTarget := ""
		if info.Mode()&os.ModeSymlink != 0 {
			linkTarget, err = os.Readlink(path)
			if err != nil {
				return err
			}
		}
		header, err := tar.FileInfoHeader(info, linkTarget)
		if err != nil {
			return err
		}
		header.Name = filepath.ToSlash(archiveName)
		if info.IsDir() {
			header.Name += "/"
		}
		if err := writer.WriteHeader(header); err != nil {
			return err
		}
		if !info.Mode().IsRegular() {
			return nil
		}

		source, err := os.Open(path)
		if err != nil {
			return err
		}
		_, copyErr := io.Copy(writer, source)
		closeErr := source.Close()
		if copyErr != nil {
			return copyErr
		}
		return closeErr
	})
}
