#!/bin/bash
# Bindings for the bits of Windows.UI.Xaml and XAML diagnostics mglass uses.
# `gen` is a small program around the windows-bindgen crate (0.62) that takes
# these windows-bindgen arguments; run from the mglass/ folder.
./target/release/gen --out src/xaml.rs --implement --no-deps \
 --reference windows_collections,flat,Windows.Foundation.Collections --reference windows_numerics,flat,Windows.Foundation.Numerics --reference windows,skip-root,Windows.Foundation --reference windows,skip-root,Windows.UI.Color --reference windows,skip-root,Windows.UI.Composition --reference windows,skip-root,Windows.UI.Core --reference windows,skip-root,Windows.Graphics --reference windows,skip-root,Windows.Win32.Foundation \
 --filter Windows.UI.Xaml.Shapes.Shape Windows.UI.Xaml.Shapes.IShape Windows.UI.Xaml.Shapes.Rectangle Windows.UI.Xaml.Shapes.IShapeStatics \
 Windows.UI.Xaml.Media.Brush Windows.UI.Xaml.Media.IBrush Windows.UI.Xaml.Media.SolidColorBrush Windows.UI.Xaml.Media.ISolidColorBrush Windows.UI.Xaml.Media.ISolidColorBrushFactory \
 Windows.UI.Xaml.Media.AcrylicBrush Windows.UI.Xaml.Media.IAcrylicBrush Windows.UI.Xaml.Media.IAcrylicBrushFactory Windows.UI.Xaml.Media.AcrylicBackgroundSource Windows.UI.Xaml.Media.XamlCompositionBrushBase Windows.UI.Xaml.Media.IXamlCompositionBrushBase \
 Windows.UI.Xaml.Hosting.ElementCompositionPreview Windows.UI.Xaml.Hosting.IElementCompositionPreviewStatics Windows.UI.Xaml.Media.VisualTreeHelper Windows.UI.Xaml.Media.IVisualTreeHelperStatics \
 Windows.UI.Xaml.FrameworkElement Windows.UI.Xaml.IFrameworkElement Windows.UI.Xaml.UIElement Windows.UI.Xaml.IUIElement \
 Windows.UI.Xaml.DependencyObject Windows.UI.Xaml.IDependencyObject Windows.UI.Xaml.IDependencyObject2 Windows.UI.Xaml.DependencyProperty Windows.UI.Xaml.IDependencyProperty Windows.UI.Xaml.DependencyPropertyChangedCallback \
 Windows.Win32.UI.Xaml.Diagnostics.IVisualTreeService Windows.Win32.UI.Xaml.Diagnostics.IXamlDiagnostics Windows.Win32.UI.Xaml.Diagnostics.IVisualTreeServiceCallback Windows.Win32.UI.Xaml.Diagnostics.IVisualTreeServiceCallback2 \
 Windows.Win32.UI.Xaml.Diagnostics.ParentChildRelation Windows.Win32.UI.Xaml.Diagnostics.VisualElement Windows.Win32.UI.Xaml.Diagnostics.VisualMutationType Windows.Win32.UI.Xaml.Diagnostics.VisualElementState Windows.Win32.UI.Xaml.Diagnostics.SourceInfo >/dev/null
