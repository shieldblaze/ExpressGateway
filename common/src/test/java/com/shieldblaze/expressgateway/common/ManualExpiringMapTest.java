package com.shieldblaze.expressgateway.common;

import org.junit.jupiter.api.Test;

import java.time.Duration;
import java.util.concurrent.ConcurrentHashMap;

import static org.junit.jupiter.api.Assertions.assertEquals;

class ManualExpiringMapTest {

    @Test
    public void testExpiring() throws Exception {
        ExpiringMap<String, String> expiringMap = new ManualExpiringMap<>(new ConcurrentHashMap<>(), Duration.ofSeconds(5), true);

        // Add entries
        for (int i = 0; i < 100; i++) {
            expiringMap.put("Meow" + i, "Cat" + i);
        }

        // Verify entries
        for (int i = 0; i < 100; i++) {
            assertEquals("Cat" + i, expiringMap.get("Meow" + i));
        }

        // Verify map size
        assertEquals(100, expiringMap.size());

        Thread.sleep(1000 * 10); // Wait for 10 seconds
        expiringMap.toString(); // Query the map to trigger cleanup

        // Verify map size
        assertEquals(0, expiringMap.size());
    }
}
