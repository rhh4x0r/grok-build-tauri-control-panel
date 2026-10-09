import SwiftUI

/// The read-aloud mini player, pinned above the composer while a reply in this thread is read.
struct MiniPlayer: View {
    private let reader = ReadAloud.shared
    /// While dragging: where the thumb is (seconds); the timeline follows it, not the audio.
    @State private var scrubbing: Double?

    var body: some View {
        let status = reader.status
        let total = max(status.estimatedTotal, 0.001)
        let position = scrubbing ?? status.position
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Image(systemName: "waveform")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(Theme.accent)
                    .symbolEffect(.variableColor.iterative, isActive: status.playing)
                Button { reader.reveal = reader.playing?.entry } label: {
                    Text(status.failed ?? reader.playing?.title ?? "")
                        .font(Theme.caption)
                        .foregroundStyle(Theme.textMuted)
                        .lineLimit(1)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .buttonStyle(.plain)
                .accessibilityHint("Shows the message being read")
                Menu {
                    ForEach(ReadAloud.rates, id: \.self) { rate in
                        Button { ReadAloud.shared.rate = rate } label: {
                            if rate == reader.rate { Label(speed(rate), systemImage: "checkmark") } else { Text(speed(rate)) }
                        }
                    }
                } label: {
                    Text(speed(reader.rate))
                        .font(Theme.mono(12, .medium))
                        .foregroundStyle(Theme.text)
                        .padding(.horizontal, 8)
                        .frame(minHeight: 26)
                        .overlay(Capsule().stroke(Theme.pillBorder, lineWidth: 1))
                }
                .accessibilityLabel("Speed")
                Button { reader.close() } label: {
                    Image(systemName: "xmark").font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.textMuted).frame(width: 26, height: 26)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Stop reading")
            }
            HStack(spacing: 10) {
                Button { reader.skip(-15) } label: { Image(systemName: "gobackward.15").font(.system(size: 17)) }
                    .buttonStyle(.plain).foregroundStyle(Theme.text).accessibilityLabel("Back 15 seconds")
                Button { reader.playPause() } label: {
                    Image(systemName: status.playing ? "pause.fill" : "play.fill")
                        .font(.system(size: 14, weight: .bold))
                        .foregroundStyle(Theme.onSolid)
                        .frame(width: 34, height: 34)
                        .background(Circle().fill(Theme.solid))
                }
                .buttonStyle(.plain)
                .accessibilityLabel(status.playing ? "Pause" : "Play")
                Button { reader.skip(15) } label: { Image(systemName: "goforward.15").font(.system(size: 17)) }
                    .buttonStyle(.plain).foregroundStyle(Theme.text).accessibilityLabel("Forward 15 seconds")
                Text(clock(position)).font(Theme.mono(11)).foregroundStyle(Theme.textFaint).monospacedDigit()
                Timeline(position: position, rendered: status.duration, total: total) { seconds, done in
                    scrubbing = done ? nil : seconds
                    if done { reader.seek(to: seconds) }
                }
                Text((status.complete || status.total == 0 ? "" : "~") + clock(total)).font(Theme.mono(11)).foregroundStyle(Theme.textFaint).monospacedDigit()
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(RoundedRectangle(cornerRadius: Theme.panelCorner).fill(Theme.glass))
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: Theme.panelCorner))
        .overlay(RoundedRectangle(cornerRadius: Theme.panelCorner).stroke(Theme.hairline, lineWidth: 1))
    }

    private func speed(_ rate: Float) -> String {
        rate == rate.rounded() ? "\(Int(rate))×" : "\(rate)×"
    }
}

/// The timeline: what's played, what's rendered (and what isn't yet), draggable to seek.
private struct Timeline: View {
    let position: Double
    let rendered: Double
    let total: Double
    let seek: (Double, Bool) -> Void

    var body: some View {
        GeometryReader { geo in
            let width = geo.size.width
            let played = CGFloat(min(max(position / total, 0), 1)) * width
            let ready = CGFloat(min(max(rendered / total, 0), 1)) * width
            ZStack(alignment: .leading) {
                Capsule().fill(Theme.bubble).frame(height: 4)
                Capsule().fill(Theme.textFaint.opacity(0.35)).frame(width: ready, height: 4)
                Capsule().fill(Theme.accent).frame(width: played, height: 4)
                Circle().fill(Theme.accent).frame(width: 12, height: 12).offset(x: played - 6)
            }
            .frame(maxHeight: .infinity)
            .contentShape(Rectangle())
            .gesture(DragGesture(minimumDistance: 0)
                .onChanged { seek(Double(min(max($0.location.x / width, 0), 1)) * total, false) }
                .onEnded { seek(Double(min(max($0.location.x / width, 0), 1)) * total, true) })
        }
        .frame(height: 28)
        .accessibilityElement()
        .accessibilityLabel("Position")
        .accessibilityValue("\(clock(position)) of \(clock(total))")
        .accessibilityAdjustableAction { direction in
            seek(min(max(position + (direction == .increment ? 15 : -15), 0), total), true)
        }
    }
}

private func clock(_ seconds: Double) -> String {
    let s = Int(max(seconds, 0))
    return "\(s / 60):" + String(format: "%02d", s % 60)
}

/// Settings → Voice: the voices with previews, speed, how to get a better voice, Personal Voice.
struct VoiceSettings: View {
    @Environment(\.dismiss) private var dismiss
    private let reader = ReadAloud.shared

    var body: some View {
        VStack(spacing: 0) {
            SheetHeader(title: "Voice") { EmptyView() } trailing: {
                Button("Done") { dismiss() }.foregroundStyle(Theme.text)
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 10) {
                    Footnote("Replies are read with your iPhone's own voices: on the phone, offline, free. Tap the speaker under a reply.")
                    SectionLabel("Speed")
                    HStack(spacing: 6) {
                        ForEach(ReadAloud.rates, id: \.self) { rate in
                            Button { ReadAloud.shared.rate = rate } label: {
                                Text(rate == rate.rounded() ? "\(Int(rate))×" : "\(rate)×")
                                    .font(Theme.mono(13, .medium))
                                    .foregroundStyle(rate == reader.rate ? Theme.onSolid : Theme.text)
                                    .frame(maxWidth: .infinity, minHeight: 34)
                                    .background(Capsule().fill(rate == reader.rate ? Theme.solid : Theme.bubble))
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    if reader.needsBetterVoice {
                        GlassCard {
                            Text("Get a better voice").font(Theme.sans(15, .semibold)).foregroundStyle(Theme.text)
                            Text("Only basic voices are installed for your language. Enhanced and Premium voices sound far more natural: Settings → Accessibility → Spoken Content → Voices, pick your language, and download one. It shows up here on its own.")
                                .font(Theme.small).foregroundStyle(Theme.textMuted).fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    Button { reader.requestPersonalVoice() } label: { Label("Use my Personal Voice", systemImage: "person.wave.2") }
                        .buttonStyle(BombButtonStyle())
                    if reader.personalVoiceAllowed == false {
                        Footnote("Not allowed, or no Personal Voice set up. Create one in Settings → Accessibility → Personal Voice.")
                    }
                    ForEach(groups, id: \.0) { group, voices in
                        SectionLabel(group).padding(.top, 6)
                        VStack(spacing: 0) {
                            ForEach(Array(voices.enumerated()), id: \.element.id) { index, voice in
                                if index > 0 { Rectangle().fill(Theme.hairline).frame(height: 1).padding(.leading, 40) }
                                VoiceRow(voice: voice, chosen: reader.chosenVoice == voice.id)
                            }
                        }
                        .background(RoundedRectangle(cornerRadius: Theme.panelCorner).fill(Theme.glass))
                        .overlay(RoundedRectangle(cornerRadius: Theme.panelCorner).stroke(Theme.hairline, lineWidth: 1))
                    }
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 24)
            }
            .scrollIndicators(.hidden)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background { BombBackground(strength: 0.5) }
        .presentationDragIndicator(.visible)
        .onAppear { reader.refreshVoices() }
    }

    /// Personal, then Premium, Enhanced, Default; the person's language first within each.
    private var groups: [(String, [SpeechVoiceInfo])] {
        let order: [(String, (SpeechVoiceInfo) -> Bool)] = [
            ("Personal Voice", { $0.personal }),
            ("Premium", { !$0.personal && $0.quality == "premium" }),
            ("Enhanced", { !$0.personal && $0.quality == "enhanced" }),
            ("Default", { !$0.personal && $0.quality == "default" }),
        ]
        return order.compactMap { name, test in
            let voices = reader.voices.filter(test)
            return voices.isEmpty ? nil : (name, voices)
        }
    }
}

private struct VoiceRow: View {
    let voice: SpeechVoiceInfo
    let chosen: Bool

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "checkmark").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.accent).opacity(chosen ? 1 : 0).frame(width: 16)
            VStack(alignment: .leading, spacing: 2) {
                Text(voice.name).font(Theme.sans(15, chosen ? .medium : .regular)).foregroundStyle(Theme.text)
                Text(Locale.current.localizedString(forIdentifier: voice.language) ?? voice.language).font(Theme.caption).foregroundStyle(Theme.textFaint)
            }
            Spacer()
            Button { ReadAloud.shared.preview(voice.id) } label: {
                Image(systemName: "play.circle").font(.system(size: 20)).foregroundStyle(Theme.textMuted)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Preview \(voice.name)")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .contentShape(Rectangle())
        .onTapGesture { ReadAloud.shared.voiceId = voice.id }
    }
}
