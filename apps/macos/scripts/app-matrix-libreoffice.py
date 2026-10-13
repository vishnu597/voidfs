# SPDX-License-Identifier: Apache-2.0
"""Run with LibreOffice's bundled Python; connects only to the test's private UNO pipe."""

import json
import sys
import time
from pathlib import Path

import uno
from com.sun.star.beans import PropertyValue


folder, phase, pipe, result = sys.argv[1:]
folder, result = Path(folder), Path(result)
report = {"phase": phase, "checks": []}


def prop(name, value):
    p = PropertyValue()
    p.Name, p.Value = name, value
    return p


def check(name, value):
    report["checks"].append({"name": name, "passed": bool(value)})
    if not value:
        raise AssertionError(name)


local = uno.getComponentContext()
resolver = local.ServiceManager.createInstanceWithContext("com.sun.star.bridge.UnoUrlResolver", local)
deadline = time.monotonic() + 30
while True:
    try:
        context = resolver.resolve(f"uno:pipe,name={pipe};urp;StarOffice.ComponentContext")
        break
    except Exception:
        if time.monotonic() > deadline:
            raise
        time.sleep(0.1)
desktop = context.ServiceManager.createInstanceWithContext("com.sun.star.frame.Desktop", context)
opened = []


def load(url):
    document = desktop.loadComponentFromURL(url, "_blank", 0, (prop("Hidden", True), prop("ReadOnly", False)))
    if document is None:
        raise RuntimeError(f"load failed: {url}")
    opened.append(document)
    return document


def metadata(document, revision):
    document.DocumentProperties.Title = "voidfs café 東京"
    document.DocumentProperties.Author = "Compatibility test"
    custom = document.DocumentProperties.UserDefinedProperties
    try:
        custom.addProperty("test_revision", 0, revision)
    except Exception:
        custom.setPropertyValue("test_revision", revision)


def metadata_ok(document, revision):
    return (document.DocumentProperties.Title == "voidfs café 東京" and
            document.DocumentProperties.Author == "Compatibility test" and
            document.DocumentProperties.UserDefinedProperties.getPropertyValue("test_revision") == revision)


try:
    for kind, filename, factory, filter_name in (
        ("Writer", "Writer café.odt", "private:factory/swriter", "writer8"),
        ("Calc", "Calc café.ods", "private:factory/scalc", "calc8"),
    ):
        path = folder / filename
        document = load(factory if phase == "create" else path.as_uri())
        if phase == "create":
            metadata(document, "one")
            if kind == "Writer":
                document.Text.String = "First draft: café 東京 — native Writer save.\nSecond paragraph."
            else:
                sheet = document.Sheets.getByIndex(0)
                sheet.Name = "Données 東京"
                sheet.getCellRangeByName("A1").Value = 7
                sheet.getCellRangeByName("A2").Value = 5
                sheet.getCellRangeByName("A3").Formula = "=SUM(A1:A2)"
                sheet.getCellRangeByName("B1").String = "café 東京"
            document.storeAsURL(path.as_uri(), (prop("FilterName", filter_name), prop("Overwrite", True)))
            check(f"{kind}: native first save", path.is_file() and not document.isModified())
        else:
            initial = phase in ("edit", "verify-initial")
            revision = "one" if initial else "two"
            check(f"{kind}: fresh-process metadata", metadata_ok(document, revision))
            check(f"{kind}: reopened writable", not document.isReadonly())
            if kind == "Writer":
                wanted = "First draft" if initial else "Edited draft"
                check("Writer: Unicode and saved text", wanted in document.Text.String and "café 東京" in document.Text.String)
            else:
                sheet = document.Sheets.getByIndex(0)
                check("Calc: Unicode sheet and cell", sheet.Name == "Données 東京" and sheet.getCellRangeByName("B1").String == "café 東京")
                check("Calc: formula and calculation", "SUM" in sheet.getCellRangeByName("A3").Formula and sheet.getCellRangeByName("A3").Value == (12 if initial else 24))
            if phase == "edit":
                metadata(document, "two")
                if kind == "Writer":
                    document.Text.String = document.Text.String.replace("First draft", "Edited draft")
                else:
                    sheet.getCellRangeByName("A1").Value = 19
                    document.calculateAll()
                document.store()
                check(f"{kind}: native edited save", not document.isModified())
            elif phase not in ("verify", "verify-initial"):
                raise ValueError(phase)
        # Record the lock while the editable document is still alive, and its cleanup on close.
        lock = folder / (".~lock." + filename + "#")
        check(f"{kind}: lock file while open", lock.is_file())
        document.close(True)
        opened.remove(document)
        check(f"{kind}: lock file removed on close", not lock.exists())
    report["passed"] = True
finally:
    for document in opened:
        try:
            document.close(True)
        except Exception:
            pass
    desktop.terminate()
    result.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
