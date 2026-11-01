// The Relais widget: the account Claude runs on and the others, read from the
// file the app writes in the group they share. Its sentences come ready-made
// from the app, in the user's language.

import SwiftUI
import WidgetKit

private let group = "BT249938WK.ai.neoservice.relais"

struct AccountLine: Codable, Identifiable {
    let alias: String
    let color: String
    let used: Double?
    let line: String
    let here: Bool
    let paused: Bool

    var id: String { alias }
}

struct Snapshot: Codable {
    let accounts: [AccountLine]
}

struct Entry: TimelineEntry {
    let date: Date
    let accounts: [AccountLine]
}

private func stored() -> [AccountLine] {
    guard let folder = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group),
        let data = try? Data(contentsOf: folder.appendingPathComponent("widget.json")),
        let snapshot = try? JSONDecoder().decode(Snapshot.self, from: data)
    else { return [] }
    return snapshot.accounts
}

private let sample = [
    AccountLine(alias: "main", color: "#4CC38A", used: 92, line: "5h 92% · 7d 38%", here: true, paused: false),
    AccountLine(alias: "spare", color: "#4C8DFF", used: 4, line: "7d 4%", here: false, paused: false),
    AccountLine(alias: "work", color: "#F2B33D", used: 55, line: "5h 30% · 7d 55%", here: false, paused: false),
]

struct Provider: TimelineProvider {
    func placeholder(in context: Context) -> Entry {
        Entry(date: .now, accounts: sample)
    }

    func getSnapshot(in context: Context, completion: @escaping (Entry) -> Void) {
        completion(Entry(date: .now, accounts: context.isPreview ? sample : stored()))
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<Entry>) -> Void) {
        let next = Date.now.addingTimeInterval(15 * 60)
        completion(Timeline(entries: [Entry(date: .now, accounts: stored())], policy: .after(next)))
    }
}

/// The design system's paper and ink, the signal as the only accent.
private func dynamic(_ light: UInt32, _ dark: UInt32) -> Color {
    func color(_ value: UInt32) -> NSColor {
        NSColor(
            srgbRed: CGFloat(value >> 16 & 255) / 255,
            green: CGFloat(value >> 8 & 255) / 255,
            blue: CGFloat(value & 255) / 255,
            alpha: 1)
    }
    return Color(
        nsColor: NSColor(name: nil) { appearance in
            appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? color(dark) : color(light)
        })
}

private let ground = dynamic(0xFAF3EA, 0x100E0D)
private let ink = dynamic(0x141414, 0xFFFDF8)
private let muted = dynamic(0x524B41, 0xBFB5A4)
private let track = dynamic(0xE8E1D6, 0x2A2420)
private let signal = Color(red: 0xF5 / 255, green: 0x5D / 255, blue: 0x1A / 255)
private let paused = Color(red: 0xFF / 255, green: 0x8D / 255, blue: 0x64 / 255)

private func dot(_ hex: String) -> Color {
    let value = UInt32(hex.dropFirst(), radix: 16) ?? 0
    return Color(
        red: Double(value >> 16 & 255) / 255,
        green: Double(value >> 8 & 255) / 255,
        blue: Double(value & 255) / 255)
}

struct Ring: View {
    let account: AccountLine
    let size: CGFloat

    private var share: Double { account.paused ? 100 : account.used ?? 0 }
    private var tint: Color { account.paused ? paused : share >= 85 ? signal : ink }

    var body: some View {
        ZStack {
            Circle().stroke(track, lineWidth: size / 9)
            Circle()
                .trim(from: 0, to: share / 100)
                .stroke(tint, style: StrokeStyle(lineWidth: size / 9, lineCap: .round))
                .rotationEffect(.degrees(-90))
            if !account.paused, let used = account.used {
                Text("\(Int(used.rounded()))")
                    .font(.system(size: size * 0.3, design: .serif))
                    .foregroundStyle(ink)
            }
        }
        .frame(width: size, height: size)
    }
}

struct Row: View {
    let account: AccountLine

    var body: some View {
        HStack(spacing: 10) {
            Ring(account: account, size: 34)
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 5) {
                    Circle().fill(dot(account.color)).frame(width: 6, height: 6)
                    Text(account.alias).font(.system(size: 13, weight: .semibold)).foregroundStyle(ink)
                }
                Text(account.line).font(.system(size: 11)).foregroundStyle(muted)
            }
            .lineLimit(1)
            Spacer(minLength: 0)
        }
    }
}

struct Small: View {
    let account: AccountLine

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Ring(account: account, size: 62)
            Spacer(minLength: 0)
            Text(account.alias).font(.system(size: 15, weight: .semibold)).foregroundStyle(ink).lineLimit(1)
            Text(account.line).font(.system(size: 11)).foregroundStyle(muted).lineLimit(2)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

struct RelaisWidgetView: View {
    @Environment(\.widgetFamily) private var family
    let entry: Entry

    var body: some View {
        Group {
            if let first = entry.accounts.first {
                if family == .systemSmall {
                    Small(account: first)
                } else {
                    VStack(alignment: .leading, spacing: 8) {
                        ForEach(entry.accounts.prefix(family == .systemMedium ? 3 : 6)) { Row(account: $0) }
                        Spacer(minLength: 0)
                    }
                }
            } else {
                Text("Relais").font(.system(size: 22, design: .serif)).foregroundStyle(ink)
            }
        }
        .containerBackground(ground, for: .widget)
    }
}

private let french = Locale.preferredLanguages.first?.hasPrefix("fr") == true

@main
struct RelaisWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "RelaisWidget", provider: Provider()) { entry in
            RelaisWidgetView(entry: entry)
        }
        .configurationDisplayName("Relais")
        .description(french ? "Les limites de tes comptes Claude." : "The limits of your Claude accounts.")
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge])
    }
}
