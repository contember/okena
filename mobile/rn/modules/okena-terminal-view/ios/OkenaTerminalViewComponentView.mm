// Fabric host for the `OkenaTerminalView` codegen component.
//
// Thin ObjC++ glue: copies props onto the Swift `TerminalRendererView`,
// forwards its event closures to the generated C++ event emitter and routes
// commands. The renderer itself lives in TerminalRendererView.swift.
//
// There is deliberately no public header: React Native finds this class by
// name (`componentProvider` in package.json), and a public header importing
// RCTViewComponentView.h would drag C++ into the pod's Swift module.

#import <React/RCTViewComponentView.h>
#import <react/renderer/components/OkenaTerminalViewSpec/ComponentDescriptors.h>
#import <react/renderer/components/OkenaTerminalViewSpec/EventEmitters.h>
#import <react/renderer/components/OkenaTerminalViewSpec/Props.h>
#import <react/renderer/components/OkenaTerminalViewSpec/RCTComponentViewHelpers.h>

#import "OkenaTerminalView-Swift.h"

using namespace facebook::react;

@interface OkenaTerminalViewComponentView : RCTViewComponentView <RCTOkenaTerminalViewViewProtocol>
@end

static NSString *OTVString(const std::string &value)
{
  return [[NSString alloc] initWithBytes:value.data() length:value.size() encoding:NSUTF8StringEncoding] ?: @"";
}

static uint32_t OTVArgb(double value)
{
  // Packed unsigned ARGB sent as a JS number; clamp out-of-range input.
  if (!(value >= 0)) return 0;
  if (value > 4294967295.0) return 0xffffffff;
  return (uint32_t)value;
}

@implementation OkenaTerminalViewComponentView {
  OkenaTerminalRendererView *_view;
}

+ (ComponentDescriptorProvider)componentDescriptorProvider
{
  return concreteComponentDescriptorProvider<OkenaTerminalViewComponentDescriptor>();
}

- (instancetype)initWithFrame:(CGRect)frame
{
  if (self = [super initWithFrame:frame]) {
    static const auto defaultProps = std::make_shared<const OkenaTerminalViewProps>();
    _props = defaultProps;
    [self installRendererView];
  }
  return self;
}

- (void)installRendererView
{
  OkenaTerminalRendererView *view = [[OkenaTerminalRendererView alloc] initWithFrame:self.bounds];
  __weak __typeof(self) weakSelf = self;

  view.onTap = ^(NSInteger col, NSInteger row) {
    auto emitter = [weakSelf terminalEventEmitter];
    if (emitter) emitter->onTap({.col = (int)col, .row = (int)row});
  };
  view.onGridSize = ^(NSInteger cols, NSInteger rows, double cellWidth, double cellHeight) {
    auto emitter = [weakSelf terminalEventEmitter];
    if (emitter) {
      emitter->onGridSize(
          {.cols = (int)cols, .rows = (int)rows, .cellWidth = cellWidth, .cellHeight = cellHeight});
    }
  };
  view.onFontSizeChange = ^(double size) {
    auto emitter = [weakSelf terminalEventEmitter];
    if (emitter) emitter->onFontSizeChange({.size = size});
  };
  view.onSelectionCopied = ^(NSString *text) {
    auto emitter = [weakSelf terminalEventEmitter];
    if (emitter) emitter->onSelectionCopied({.text = std::string(text.UTF8String ?: "")});
  };
  view.onScrollStateChange = ^(BOOL atBottom) {
    auto emitter = [weakSelf terminalEventEmitter];
    if (emitter) emitter->onScrollStateChange({.atBottom = (bool)atBottom});
  };
  view.onOutputPaused = ^(BOOL paused) {
    auto emitter = [weakSelf terminalEventEmitter];
    if (emitter) emitter->onOutputPausedChange({.paused = (bool)paused});
  };

  _view = view;
  self.contentView = view;
}

- (std::shared_ptr<const OkenaTerminalViewEventEmitter>)terminalEventEmitter
{
  if (!_eventEmitter) return nullptr;
  return std::static_pointer_cast<const OkenaTerminalViewEventEmitter>(_eventEmitter);
}

- (void)updateProps:(Props::Shared const &)props oldProps:(Props::Shared const &)oldProps
{
  const auto &newProps = *std::static_pointer_cast<const OkenaTerminalViewProps>(props);

  // The Swift setters ignore unchanged values, so set everything: after a
  // recycle the old props are not the ones the view last saw.
  _view.connId = OTVString(newProps.connId);
  _view.terminalId = OTVString(newProps.terminalId);
  if (newProps.fontSize > 0) _view.fontSize = newProps.fontSize;
  _view.backgroundArgb = OTVArgb(newProps.theme.background);
  _view.foregroundArgb = OTVArgb(newProps.theme.foreground);
  _view.cursorArgb = OTVArgb(newProps.theme.cursor);
  _view.selectionArgb = OTVArgb(newProps.theme.selection);
  _view.focusedPane = newProps.focused;

  [super updateProps:props oldProps:oldProps];
}

- (void)prepareForRecycle
{
  [super prepareForRecycle];
  // A recycled view may next show another terminal: start from a fresh
  // renderer rather than resetting its state field by field.
  [self installRendererView];
}

#pragma mark - Commands

- (void)handleCommand:(const NSString *)commandName args:(const NSArray *)args
{
  RCTOkenaTerminalViewHandleCommand(self, commandName, args);
}

- (void)scrollToBottom
{
  [_view scrollToBottom];
}

- (void)clearSelection
{
  [_view clearSelection];
}

@end
