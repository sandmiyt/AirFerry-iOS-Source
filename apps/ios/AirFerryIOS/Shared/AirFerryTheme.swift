import SwiftUI

enum AirFerryTheme {
    static let accent = Color(red: 0.08, green: 0.48, blue: 0.96)
    static let cyan = Color(red: 0.06, green: 0.82, blue: 0.91)
    static let violet = Color(red: 0.45, green: 0.35, blue: 0.96)
}

struct AirFerryBackdrop: View {
    var body: some View {
        ZStack {
            LinearGradient(
                colors: [
                    Color(uiColor: .systemBackground),
                    AirFerryTheme.accent.opacity(0.10),
                    AirFerryTheme.cyan.opacity(0.08)
                ],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )

            Circle()
                .fill(AirFerryTheme.cyan.opacity(0.20))
                .frame(width: 300, height: 300)
                .blur(radius: 78)
                .offset(x: 160, y: -260)

            Circle()
                .fill(AirFerryTheme.violet.opacity(0.14))
                .frame(width: 280, height: 280)
                .blur(radius: 84)
                .offset(x: -170, y: 300)
        }
        .ignoresSafeArea()
        .accessibilityHidden(true)
    }
}

struct AirFerryGlassGroup<Content: View>: View {
    private let spacing: CGFloat
    private let content: Content

    init(spacing: CGFloat = 16, @ViewBuilder content: () -> Content) {
        self.spacing = spacing
        self.content = content()
    }

    @ViewBuilder
    var body: some View {
        if #available(iOS 26.0, *) {
            GlassEffectContainer(spacing: spacing) {
                content
            }
        } else {
            content
        }
    }
}

extension View {
    @ViewBuilder
    func airFerryGlassCard(
        cornerRadius: CGFloat = 26,
        interactive: Bool = false
    ) -> some View {
        if #available(iOS 26.0, *) {
            if interactive {
                self.glassEffect(
                    .regular.tint(AirFerryTheme.accent.opacity(0.10)).interactive(),
                    in: .rect(cornerRadius: cornerRadius)
                )
            } else {
                self.glassEffect(
                    .regular.tint(AirFerryTheme.accent.opacity(0.06)),
                    in: .rect(cornerRadius: cornerRadius)
                )
            }
        } else {
            self
                .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
                .overlay {
                    RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                        .stroke(.white.opacity(0.24), lineWidth: 0.8)
                }
        }
    }

    @ViewBuilder
    func airFerryPrimaryButton() -> some View {
        if #available(iOS 26.0, *) {
            self.buttonStyle(.glassProminent)
        } else {
            self.buttonStyle(.borderedProminent)
        }
    }

    @ViewBuilder
    func airFerrySecondaryButton() -> some View {
        if #available(iOS 26.0, *) {
            self.buttonStyle(.glass)
        } else {
            self.buttonStyle(.bordered)
        }
    }
}
