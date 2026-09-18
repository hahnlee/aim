#pragma once

#import <AppKit/AppKit.h>
#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>

struct DarwinArtSurface;

// AppKit content view owned by one DarwinArtSurface.  The view owns its
// CAMetalLayer and translates host events into the surface's immutable input
// sink; Android window/focus policy remains outside this provider.
@interface DarwinArtMetalView : NSView
@property(nonatomic, readonly) CAMetalLayer* metalLayer;
- (instancetype)initWithFrame:(NSRect)frame
                       device:(id<MTLDevice>)device
                    pixelSize:(CGSize)pixelSize
                 contentScale:(CGFloat)contentScale;
- (void)cancelPointerStream;
- (void)updateDrawableSize;
- (void)setOwnerSurface:(DarwinArtSurface*)surface;
- (DarwinArtSurface*)ownerSurface;
- (void)signalOwnerWake;
@end
