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
package com.shieldblaze.expressgateway.healthcheck;

import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.List;

import static org.junit.jupiter.api.Assertions.*;

class CompositeHealthCheckTest {

    private static final InetSocketAddress ADDR = new InetSocketAddress("127.0.0.1", 1);
    private static final Duration TIMEOUT = Duration.ofSeconds(5);

    static class AlwaysHealthy extends HealthCheck {
        AlwaysHealthy() {
            super(ADDR, TIMEOUT, 10);
        }

        @Override
        public void run() {
            markSuccess();
        }
    }

    static class AlwaysFailing extends HealthCheck {
        AlwaysFailing() {
            super(ADDR, TIMEOUT, 10);
        }

        @Override
        public void run() {
            markFailure();
        }
    }

    @Test
    void allComponentsHealthy_compositeIsHealthy() {
        CompositeHealthCheck composite = new CompositeHealthCheck(
                ADDR, TIMEOUT, 10,
                List.of(new AlwaysHealthy(), new AlwaysHealthy())
        );

        composite.run();

        assertEquals(Health.GOOD, composite.health());
    }

    @Test
    void oneComponentFailing_compositeIsBad() {
        CompositeHealthCheck composite = new CompositeHealthCheck(
                ADDR, TIMEOUT, 10,
                List.of(new AlwaysHealthy(), new AlwaysFailing())
        );

        composite.run();

        assertEquals(Health.BAD, composite.health());
    }

    @Test
    void allComponentsFailing_compositeIsBad() {
        CompositeHealthCheck composite = new CompositeHealthCheck(
                ADDR, TIMEOUT, 10,
                List.of(new AlwaysFailing(), new AlwaysFailing())
        );

        composite.run();

        assertEquals(Health.BAD, composite.health());
    }

    @Test
    void emptyComponents_throwsException() {
        assertThrows(IllegalArgumentException.class,
                () -> new CompositeHealthCheck(ADDR, TIMEOUT, 10, List.of()));
    }

    @Test
    void componentsAccessor() {
        AlwaysHealthy h1 = new AlwaysHealthy();
        AlwaysHealthy h2 = new AlwaysHealthy();

        CompositeHealthCheck composite = new CompositeHealthCheck(
                ADDR, TIMEOUT, 10, List.of(h1, h2));

        assertEquals(2, composite.components().size());
    }

    @Test
    void compositeWithRiseFall() {
        CompositeHealthCheck composite = new CompositeHealthCheck(
                ADDR, TIMEOUT, 10, 2, 2,
                List.of(new AlwaysHealthy(), new AlwaysHealthy())
        );

        // First run: 1 success, but rise=2 so still UNKNOWN
        composite.run();
        assertEquals(Health.UNKNOWN, composite.health());

        // Second run: 2 consecutive successes, meets rise threshold
        composite.run();
        assertEquals(Health.GOOD, composite.health());
    }
}
