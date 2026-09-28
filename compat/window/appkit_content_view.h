#pragma once

#import <AppKit/AppKit.h>
#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>

struct AimSurface;

// AppKit content view owned by one AimSurface.  The view owns its
// CAMetalLayer and translates host events into the surface's immutable input
// sink; Android window/focus policy remains outside this provider.
@interface AimMetalView : NSView
@property(nonatomic, readonly) CAMetalLayer* metalLayer;
- (instancetype)initWithFrame:(NSRect)frame
                       device:(id<MTLDevice>)device
                    pixelSize:(CGSize)pixelSize
                 contentScale:(CGFloat)contentScale;
- (void)cancelPointerStream;
- (void)updateDrawableSize;
- (void)setOwnerSurface:(AimSurface*)surface;
- (AimSurface*)ownerSurface;
- (void)signalOwnerWake;
@end
