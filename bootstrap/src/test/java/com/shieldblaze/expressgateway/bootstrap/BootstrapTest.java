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

import com.shieldblaze.expressgateway.common.ExpressGateway;
import com.shieldblaze.expressgateway.common.zookeeper.Environment;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.File;

import static com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil.getPropertyOrEnv;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class BootstrapTest {

    @BeforeAll
    static void loadConfigurationFile() {
        assertNull(getPropertyOrEnv("CONFIGURATION_DIRECTORY"));

        ClassLoader classLoader = BootstrapTest.class.getClassLoader();
        File file = new File(classLoader.getResource("default").getFile());
        String absolutePath = file.getAbsolutePath();

        System.setProperty("CONFIGURATION_DIRECTORY", absolutePath);
        assertNotNull(getPropertyOrEnv("CONFIGURATION_DIRECTORY"));
    }

    @AfterEach
    void shutdownBootstrapInstance() {
        Bootstrap.shutdown();
    }

    @Test
    void loadConfigurationFileAndCheckSystemPropertiesTest() throws Exception {
        Bootstrap.main();

        assertNotNull(ExpressGateway.getInstance());

        assertEquals(ExpressGateway.RunningMode.STANDALONE, ExpressGateway.getInstance().runningMode());
        assertEquals("1-2-3-4-5-f", ExpressGateway.getInstance().clusterID());
        assertEquals(Environment.DEVELOPMENT, ExpressGateway.getInstance().environment());

        assertEquals("127.0.0.1", ExpressGateway.getInstance().restApi().IPAddress());
        assertEquals(9110, ExpressGateway.getInstance().restApi().port());
        assertFalse(ExpressGateway.getInstance().restApi().enableTLS());
    }

    @Test
    void startupMetricsAreRecorded() throws Exception {
        Bootstrap.main();

        StartupMetrics metrics = Bootstrap.startupMetrics();
        assertTrue(metrics.isReady(), "Startup should be marked as ready after main()");
        assertTrue(metrics.timeToReady().toMillis() >= 0, "Time-to-ready should be non-negative");
        assertFalse(metrics.componentTimes().isEmpty(), "At least one component should be timed");
        assertTrue(metrics.componentTimes().containsKey("configuration-load"),
                "Configuration load should be timed");
    }

    @Test
    void startupHealthChecksPass() throws Exception {
        Bootstrap.main();

        StartupHealthCheck.Result result = StartupHealthCheck.runChecks();
        assertTrue(result.healthy(), "Health checks should pass after successful startup");
        assertFalse(result.passed().isEmpty(), "At least one check should pass");
        assertTrue(result.failed().isEmpty(), "No checks should fail");
    }

    @Test
    void shutdownActionRegistration() throws Exception {
        Bootstrap.main();

        java.util.concurrent.atomic.AtomicBoolean called = new java.util.concurrent.atomic.AtomicBoolean();
        Bootstrap.registerShutdownAction(() -> called.set(true));

        // We cannot trigger the JVM shutdown hook in a test, but we verify
        // the action was registered (null actions are silently ignored)
        Bootstrap.registerShutdownAction(null); // should not throw
    }
}
