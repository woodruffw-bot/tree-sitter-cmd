import XCTest
import SwiftTreeSitter
import TreeSitterCmd

final class TreeSitterCmdTests: XCTestCase {
    func testCanLoadGrammar() throws {
        let parser = Parser()
        let language = Language(language: tree_sitter_cmd())
        XCTAssertNoThrow(try parser.setLanguage(language),
                         "Error loading Cmd grammar")
    }
}
