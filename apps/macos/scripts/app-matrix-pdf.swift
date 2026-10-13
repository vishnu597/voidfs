// SPDX-License-Identifier: Apache-2.0
// Semantic readback of the PDF annotated/exported in Preview, on mount or downloaded server copy.
import Foundation
import PDFKit

let paths = Array(CommandLine.arguments.dropFirst())
guard !paths.isEmpty else { fputs("app-matrix-pdf <pdf>...\n", stderr); exit(1) }
var failed = false
for path in paths {
    guard let document = PDFDocument(url: URL(fileURLWithPath: path)) else { fputs("unreadable PDF: \(path)\n", stderr); exit(1) }
    let annotations = (0..<document.pageCount).flatMap { document.page(at: $0)?.annotations.compactMap(\.contents) ?? [] }
    let text = document.string ?? ""
    let ok = document.pageCount == 1 && text.contains("voidfs application matrix original PDF") && annotations.contains("voidfs saved annotation — 名前")
    let result: [String: Any] = ["path": path, "pages": document.pageCount, "text": text, "annotations": annotations, "ok": ok]
    print(String(decoding: try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]), as: UTF8.self))
    failed = failed || !ok
}
exit(failed ? 1 : 0)
