/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.controlplane.region;

import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.time.Duration;
import java.time.Instant;
import java.util.ArrayList;
import java.util.List;
import java.util.Optional;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class RegionFailoverPolicyTest {

    private RegionManager regionManager;
    private RegionFailoverPolicy policy;
    private List<RegionFailoverPolicy.FailoverEvent> events;

    @BeforeEach
    void setUp() {
        regionManager = new RegionManager("us-east-1");
        regionManager.updateRegion(new RegionManager.RegionState(
                "eu-west-1", RegionManager.RegionHealth.HEALTHY, Instant.now(), 2));
        regionManager.updateRegion(new RegionManager.RegionState(
                "ap-south-1", RegionManager.RegionHealth.HEALTHY, Instant.now(), 2));

        events = new ArrayList<>();
        policy = new RegionFailoverPolicy(
                regionManager,
                List.of("eu-west-1", "ap-south-1"),
                Duration.ofMillis(100), // 100ms drain timeout for tests
                Duration.ofSeconds(0),  // no cooldown for tests
                true
        );
        policy.addListener(events::add);
    }

    @Test
    void failoverSelectsFirstPriorityTarget() {
        Optional<String> target = policy.evaluateFailover("us-east-1");
        assertTrue(target.isPresent());
        assertEquals("eu-west-1", target.get());
    }

    @Test
    void failoverSkipsUnhealthyPriorityTargets() {
        regionManager.updateRegion(new RegionManager.RegionState(
                "eu-west-1", RegionManager.RegionHealth.UNHEALTHY, Instant.now(), 0));

        Optional<String> target = policy.evaluateFailover("us-east-1");
        assertTrue(target.isPresent());
        assertEquals("ap-south-1", target.get());
    }

    @Test
    void failoverReturnsEmptyWhenNoTargetsAvailable() {
        regionManager.updateRegion(new RegionManager.RegionState(
                "eu-west-1", RegionManager.RegionHealth.UNHEALTHY, Instant.now(), 0));
        regionManager.updateRegion(new RegionManager.RegionState(
                "ap-south-1", RegionManager.RegionHealth.UNHEALTHY, Instant.now(), 0));

        Optional<String> target = policy.evaluateFailover("us-east-1");
        assertTrue(target.isEmpty());
    }

    @Test
    void duplicateFailoverReturnEmpty() {
        Optional<String> first = policy.evaluateFailover("us-east-1");
        assertTrue(first.isPresent());

        Optional<String> second = policy.evaluateFailover("us-east-1");
        assertTrue(second.isEmpty()); // Already failed over
    }

    @Test
    void failbackWhenOriginalRegionRecovers() {
        policy.evaluateFailover("us-east-1");
        assertTrue(policy.activeFailoverTarget("us-east-1").isPresent());

        // Original region is healthy (it was never actually marked unhealthy in the manager)
        boolean failedBack = policy.evaluateFailback("us-east-1");
        assertTrue(failedBack);
        assertTrue(policy.activeFailoverTarget("us-east-1").isEmpty());
    }

    @Test
    void noFailbackWhenAutoFailbackDisabled() {
        RegionFailoverPolicy noFailbackPolicy = new RegionFailoverPolicy(
                regionManager,
                List.of("eu-west-1"),
                Duration.ofMillis(100),
                Duration.ofSeconds(0),
                false // disabled
        );

        noFailbackPolicy.evaluateFailover("us-east-1");
        boolean failedBack = noFailbackPolicy.evaluateFailback("us-east-1");
        assertFalse(failedBack);
    }

    @Test
    void failoverEventsFireCorrectly() {
        policy.evaluateFailover("us-east-1");

        // Should have: DRAIN_STARTED, DRAIN_COMPLETED, FAILOVER
        assertEquals(3, events.size());
        assertEquals(RegionFailoverPolicy.FailoverType.DRAIN_STARTED, events.get(0).type());
        assertEquals(RegionFailoverPolicy.FailoverType.DRAIN_COMPLETED, events.get(1).type());
        assertEquals(RegionFailoverPolicy.FailoverType.FAILOVER, events.get(2).type());
    }

    @Test
    void drainActuallyWaits() throws Exception {
        // Use a longer drain timeout to verify the drain actually delays
        RegionFailoverPolicy slowDrainPolicy = new RegionFailoverPolicy(
                regionManager,
                List.of("eu-west-1"),
                Duration.ofMillis(200), // 200ms drain timeout
                Duration.ofSeconds(0),
                true
        );
        List<RegionFailoverPolicy.FailoverEvent> drainEvents = new ArrayList<>();
        slowDrainPolicy.addListener(drainEvents::add);

        long start = System.nanoTime();
        CompletableFuture<Optional<String>> future = slowDrainPolicy.evaluateFailoverAsync("us-east-1");
        Optional<String> result = future.get(5, TimeUnit.SECONDS);
        long elapsed = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - start);

        assertTrue(result.isPresent());
        // Elapsed time should be at least 150ms (giving some margin for scheduling)
        assertTrue(elapsed >= 150, "Drain should have waited ~200ms, but only waited " + elapsed + "ms");

        // Verify events: DRAIN_STARTED fires before DRAIN_COMPLETED
        assertTrue(drainEvents.size() >= 3);
        assertEquals(RegionFailoverPolicy.FailoverType.DRAIN_STARTED, drainEvents.get(0).type());
        assertEquals(RegionFailoverPolicy.FailoverType.DRAIN_COMPLETED, drainEvents.get(1).type());
        assertEquals(RegionFailoverPolicy.FailoverType.FAILOVER, drainEvents.get(2).type());
    }

    @Test
    void concurrentEvaluateFailoverProducesOnlyOneFailover() throws Exception {
        // Launch multiple concurrent evaluateFailover calls for the same region
        AtomicInteger successCount = new AtomicInteger(0);
        int threadCount = 10;
        CountDownLatch startLatch = new CountDownLatch(1);
        CountDownLatch doneLatch = new CountDownLatch(threadCount);

        ExecutorService executor = Executors.newFixedThreadPool(threadCount);
        for (int i = 0; i < threadCount; i++) {
            executor.submit(() -> {
                try {
                    startLatch.await();
                    Optional<String> result = policy.evaluateFailover("us-east-1");
                    if (result.isPresent()) {
                        successCount.incrementAndGet();
                    }
                } catch (Exception e) {
                    // ignored
                } finally {
                    doneLatch.countDown();
                }
            });
        }

        startLatch.countDown();
        doneLatch.await(5, TimeUnit.SECONDS);
        executor.shutdown();

        // Only one thread should have successfully initiated the failover
        assertEquals(1, successCount.get(),
                "Only one concurrent evaluateFailover should produce a failover");
    }

    @Test
    void cooldownPreventsRapidFailover() {
        RegionFailoverPolicy cooldownPolicy = new RegionFailoverPolicy(
                regionManager,
                List.of("eu-west-1"),
                Duration.ofMillis(10),
                Duration.ofHours(1), // 1 hour cooldown
                true
        );

        Optional<String> first = cooldownPolicy.evaluateFailover("us-east-1");
        assertTrue(first.isPresent());

        // Clear the active failover so we can try again
        cooldownPolicy.evaluateFailback("us-east-1");

        // Second failover should be blocked by cooldown
        Optional<String> second = cooldownPolicy.evaluateFailover("us-east-1");
        assertTrue(second.isEmpty());
    }

    @Test
    void allActiveFailoversReturnsMapping() {
        policy.evaluateFailover("us-east-1");
        var failovers = policy.allActiveFailovers();
        assertEquals(1, failovers.size());
        assertEquals("eu-west-1", failovers.get("us-east-1"));
    }
}
