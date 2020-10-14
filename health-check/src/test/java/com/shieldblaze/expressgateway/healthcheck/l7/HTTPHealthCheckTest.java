/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.healthcheck.l7;

import com.shieldblaze.expressgateway.healthcheck.Health;
import org.junit.jupiter.api.Test;

import java.net.URI;
import java.time.Duration;

import static org.junit.jupiter.api.Assertions.assertEquals;

final class HTTPHealthCheckTest {

    @Test
    void testPass() throws Exception {
        new HTTPServer("200 OK").start();

        Thread.sleep(1000L);

        HTTPHealthCheck httpHealthCheck = new HTTPHealthCheck(URI.create("http://127.0.0.1:9111"), Duration.ofSeconds(5), false);
        httpHealthCheck.run();

        assertEquals(Health.GOOD, httpHealthCheck.health());
    }

    @Test
    void testFail() throws Exception {
        new HTTPServer("500 Internal Server Error").start();

        Thread.sleep(1000L);

        HTTPHealthCheck httpHealthCheck = new HTTPHealthCheck(URI.create("http://127.0.0.1:9111"), Duration.ofSeconds(5), false);
        httpHealthCheck.run();

        assertEquals(Health.BAD, httpHealthCheck.health());
    }
}
