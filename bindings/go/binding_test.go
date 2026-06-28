package tree_sitter_cmd_test

import (
	"testing"

	tree_sitter "github.com/tree-sitter/go-tree-sitter"
	tree_sitter_cmd "github.com/woodruffw-bot/tree-sitter-cmd/bindings/go"
)

func TestCanLoadGrammar(t *testing.T) {
	language := tree_sitter.NewLanguage(tree_sitter_cmd.Language())
	if language == nil {
		t.Errorf("Error loading Cmd grammar")
	}
}
