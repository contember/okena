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
  s.pod_target_xcconfig = { 'DEFINES_MODULE' => 'YES' }

  install_modules_dependencies(s)
end
