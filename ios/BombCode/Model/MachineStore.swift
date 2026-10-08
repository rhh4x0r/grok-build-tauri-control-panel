import Foundation
import Security

/// Paired machines, kept in the Keychain because each holds this phone's private key for that machine.
enum MachineStore {
    private static let service = "com.bombcode.companion.machines"

    static func load() -> [PairedMachine] {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecMatchLimit as String: kSecMatchLimitAll,
            kSecReturnData as String: true,
        ]
        var result: AnyObject?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess, let items = result as? [Data] else { return [] }
        return items.compactMap(decode).sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }

    /// False when the Keychain refused it (an unsigned build, for one); the pairing then lasts only until the app quits.
    @discardableResult
    static func save(_ machine: PairedMachine) -> Bool {
        delete(id: machine.id)
        let item: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: machine.id,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
            kSecValueData as String: encode(machine),
        ]
        return SecItemAdd(item as CFDictionary, nil) == errSecSuccess
    }

    static func delete(id: String) {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: id,
        ]
        SecItemDelete(query as CFDictionary)
    }

    private static func encode(_ m: PairedMachine) -> Data {
        let object: [String: Any] = [
            "id": m.id, "name": m.name, "host": m.host, "fingerprint": m.fingerprint, "deviceId": m.deviceId,
            "user": m.user, "admin": m.admin, "kind": m.kind, "cert": m.identity.certPem, "key": m.identity.keyPem,
        ]
        return (try? JSONSerialization.data(withJSONObject: object)) ?? Data()
    }

    private static func decode(_ data: Data) -> PairedMachine? {
        guard let o = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let id = o["id"] as? String, let host = o["host"] as? String, let fingerprint = o["fingerprint"] as? String,
              let cert = o["cert"] as? String, let key = o["key"] as? String else { return nil }
        return PairedMachine(
            id: id, name: o["name"] as? String ?? host, host: host, fingerprint: fingerprint,
            deviceId: o["deviceId"] as? String ?? "", user: o["user"] as? String ?? "", admin: o["admin"] as? Bool ?? false,
            kind: o["kind"] as? String ?? "server", identity: DeviceIdentity(certPem: cert, keyPem: key)
        )
    }
}
