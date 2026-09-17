package dev.lookhin.flyminer;

/** Explicit in-game consent is tied to one world/server, player and dimension. */
final class SessionGuard {
    private String authorized;

    void enable(String session) {
        if (session == null || session.isBlank()) throw new IllegalArgumentException("No active world");
        authorized = session;
    }

    boolean allows(String session) {
        return authorized != null && authorized.equals(session);
    }

    void clear() { authorized = null; }
}
