import SwiftUI

struct RootView: View {
    @StateObject private var sender = SenderViewModel()
    @StateObject private var receiver = ReceiverViewModel()

    var body: some View {
        TabView {
            NavigationStack {
                SenderView(model: sender)
            }
            .tabItem { Label("发送", systemImage: "arrow.up.circle.fill") }

            NavigationStack {
                ReceiverView(model: receiver)
            }
            .tabItem { Label("接收", systemImage: "viewfinder.circle.fill") }
        }
        .tint(AirFerryTheme.accent)
    }
}

#Preview {
    RootView()
}
