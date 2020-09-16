package com.shieldblaze.expressgateway.core.configuration.eventloop;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertThrows;

class EventLoopConfigurationBuilderTest {

    @Test
    void build() {
        assertThrows(IllegalArgumentException.class, () -> EventLoopConfigurationBuilder.newBuilder()
                .withParentWorkers(0)
                .build());

        assertThrows(IllegalArgumentException.class, () -> EventLoopConfigurationBuilder.newBuilder()
                .withParentWorkers(1)
                .build());

        assertThrows(IllegalArgumentException.class, () -> EventLoopConfigurationBuilder.newBuilder()
                .withParentWorkers(1)
                .withChildWorkers(0)
                .build());

        assertDoesNotThrow(() -> EventLoopConfigurationBuilder.newBuilder()
                .withParentWorkers(1)
                .withChildWorkers(1)
                .build());
    }
}
