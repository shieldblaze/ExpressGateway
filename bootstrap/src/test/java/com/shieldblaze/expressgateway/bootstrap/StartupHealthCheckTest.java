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
package com.shieldblaze.expressgateway.bootstrap;

import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class StartupHealthCheckTest {

    @Test
    void allChecksPassing() {
        List<StartupHealthCheck.Check> checks = List.of(
                new StartupHealthCheck.Check("check-1", () -> true),
                new StartupHealthCheck.Check("check-2", () -> true)
        );

        StartupHealthCheck.Result result = StartupHealthCheck.executeChecks(checks);
        assertTrue(result.healthy());
        assertEquals(2, result.passed().size());
        assertTrue(result.failed().isEmpty());
    }

    @Test
    void failingCheck() {
        List<StartupHealthCheck.Check> checks = List.of(
                new StartupHealthCheck.Check("passes", () -> true),
                new StartupHealthCheck.Check("fails", () -> false)
        );

        StartupHealthCheck.Result result = StartupHealthCheck.executeChecks(checks);
        assertFalse(result.healthy());
        assertEquals(1, result.passed().size());
        assertEquals(1, result.failed().size());
        assertEquals("fails", result.failed().getFirst());
    }

    @Test
    void exceptionTreatedAsFailure() {
        List<StartupHealthCheck.Check> checks = List.of(
                new StartupHealthCheck.Check("throws", () -> { throw new RuntimeException("boom"); })
        );

        StartupHealthCheck.Result result = StartupHealthCheck.executeChecks(checks);
        assertFalse(result.healthy());
        assertEquals(1, result.failed().size());
        assertTrue(result.failed().getFirst().contains("boom"));
    }

    @Test
    void emptyChecksAreHealthy() {
        StartupHealthCheck.Result result = StartupHealthCheck.executeChecks(List.of());
        assertTrue(result.healthy());
    }
}
