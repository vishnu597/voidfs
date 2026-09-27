// SPDX-License-Identifier: Apache-2.0

import ExtensionFoundation
import FSKit

@main
struct VoidfsExtension: UnaryFileSystemExtension {
    let fileSystem = VoidfsFileSystem()
}
