// SPDX-License-Identifier: Apache-2.0
// Native framework reads of the two synthetic GUI exports; no write or app launch.
import CryptoKit
import Foundation
import PDFKit

guard CommandLine.arguments.count == 3 else { fputs("read-exports <preview.pdf> <numbers.csv>\n", stderr); exit(2) }
func emit(_ record: [String: Any]) {
    let data = try! JSONSerialization.data(withJSONObject: record, options: [.sortedKeys])
    print(String(decoding: data, as: UTF8.self))
}
func sha(_ data: Data) -> String { SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() }
do {
    let pdfURL = URL(fileURLWithPath: CommandLine.arguments[1])
    let pdfBytes = try Data(contentsOf: pdfURL)
    guard let pdf = PDFDocument(url: pdfURL), pdf.pageCount > 0 else { throw NSError(domain: "ExportRead", code: 1, userInfo: [NSLocalizedDescriptionKey: "PDFKit could not open a page"]) }
    let text = (0..<pdf.pageCount).compactMap { pdf.page(at: $0)?.string }.joined(separator: "\n")
    emit(["kind": "native-read", "reader": "PDFKit", "path": pdfURL.path, "pages": pdf.pageCount, "bytes": pdfBytes.count, "sha256": sha(pdfBytes), "extractedText": text, "ok": true])
    let csvURL = URL(fileURLWithPath: CommandLine.arguments[2])
    let csvBytes = try Data(contentsOf: csvURL)
    let csv = try String(contentsOf: csvURL, encoding: .utf8)
    let valid = csv.hasPrefix("Item,Amount,Twice") && csv.contains("名前,22")
    emit(["kind": "native-read", "reader": "Foundation UTF-8", "path": csvURL.path, "bytes": csvBytes.count, "sha256": sha(csvBytes), "text": csv, "ok": valid])
    exit(valid ? 0 : 1)
} catch {
    emit(["kind": "error", "error": String(describing: error)])
    exit(1)
}
