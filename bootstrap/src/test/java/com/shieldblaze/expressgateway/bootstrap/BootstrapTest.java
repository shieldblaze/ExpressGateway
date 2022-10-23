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
}
