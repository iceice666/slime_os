# Public recipes are grouped by responsibility in imported files. Imports keep
# every historical top-level recipe name stable while making ownership visible.
import 'just/product.just'
import 'just/planes-mechanism.just'
import 'just/planes-runtime.just'
import 'just/planes-fabric.just'
import 'just/planes-storage.just'
import 'just/contracts.just'
import 'just/generate.just'
import 'just/component-sdk.just'
import 'just/hardware.just'
import 'just/quality.just'
import 'just/compatibility.just'

[group('navigation')]
[private]
help:
    @just --choose
