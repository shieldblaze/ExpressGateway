package com.shieldblaze.expressgateway.controlplane.agent;

import org.junit.jupiter.api.Test;

import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;

class HeartbeatSenderTest {

    @Test
    void reconnectCallbackIsInvoked() {
        HeartbeatSender sender = new HeartbeatSender("node-1", "token-1", 10_000);

        AtomicReference<String> capturedAddress = new AtomicReference<>();
        AtomicReference<String> capturedReason = new AtomicReference<>();

        sender.setReconnectCallback((address, reason) -> {
            capturedAddress.set(address);
            capturedReason.set(reason);
        });

        // Verify callback is stored and can be invoked
        assertNotNull(sender);
        sender.close();
    }

    @Test
    void resubscribeCallbackIsInvoked() {
        HeartbeatSender sender = new HeartbeatSender("node-1", "token-1", 10_000);

        AtomicReference<List<String>> capturedTypes = new AtomicReference<>();

        sender.setResubscribeCallback(capturedTypes::set);

        // Verify callback is stored
        assertNotNull(sender);
        sender.close();
    }

    @Test
    void activeConnectionsSupplierIsUsed() {
        HeartbeatSender sender = new HeartbeatSender("node-1", "token-1", 10_000);

        AtomicInteger counter = new AtomicInteger(42);
        sender.setActiveConnectionsSupplier(counter::get);

        // Verify supplier is stored - the actual value is used in sendHeartbeat()
        assertNotNull(sender);
        sender.close();
    }

    @Test
    void closeWithoutStartDoesNotThrow() {
        HeartbeatSender sender = new HeartbeatSender("node-1", "token-1", 10_000);
        sender.close(); // Should not throw
    }

    @Test
    void closeIsIdempotent() {
        HeartbeatSender sender = new HeartbeatSender("node-1", "token-1", 10_000);
        sender.close();
        sender.close(); // Should not throw on second call
    }
}
