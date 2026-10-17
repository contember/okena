require 'json'

package = JSON.parse(File.read(File.join(__dir__, 'package.json')))

Pod::Spec.new do |s|
  s.name         = 'OkenaTerminalView'
  s.version      = package['version']
  s.summary      = package['description']
  s.license      = package['license']
  s.authors      = package['author']
  s.homepage     = package['homepage']
  s.platforms    = { :ios => min_ios_version_supported }
  s.source       = { :git => 'https://github.com/contember/okena.git', :tag => "v#{s.version}" }

  # Swift renderer view + ObjC++ Fabric wrapper. The wrapper imports the
  # generated `OkenaTerminalView-Swift.h`, which needs a module.
  s.source_files = 'ios/**/*.{h,m,mm,swift}'
  s.swift_version = '5.9'
  # The Rust render ABI (okena_render.h + module.modulemap, Clang module
  # `OkenaRender`). Only headers: the symbols resolve at app link time from
  # the one Rust copy in the OkenaMobileFfi pod.
  # Relative to the pod root (node_modules/okena-terminal-view or
  # modules/okena-terminal-view, both four levels below the repo root). An
  # absolute path here would make the Podfile.lock checksum machine-specific.
  okena_render_include = '$(PODS_TARGET_SRCROOT)/../../../../crates/okena-mobile-ffi/include'
  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    'SWIFT_INCLUDE_PATHS' => "\"#{okena_render_include}\"",
    'HEADER_SEARCH_PATHS' => "\"#{okena_render_include}\"",
  }

  install_modules_dependencies(s)
end
