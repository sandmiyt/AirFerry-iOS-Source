import SwiftUI

struct RootView: View {
    @StateObject private var sender = SenderViewModel()
    @StateObject private var receiver = ReceiverViewModel()

    var body: some View {
        TabView {
            NavigationStack {
                SenderView(model: sender)
            }
            .tabItem { Label("发送", systemImage: "qrcode") }

            NavigationStack {
                ReceiverView(model: receiver)
            }
            .tabItem { Label("接收", systemImage: "viewfinder") }
        }
        .tint(.indigo)
    }
}

#Preview {
    RootView()
}

