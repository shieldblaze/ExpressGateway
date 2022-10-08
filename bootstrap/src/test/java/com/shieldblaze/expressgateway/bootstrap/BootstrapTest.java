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

import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.io.File;

import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CLUSTER_ID;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CONFIGURATION_DIRECTORY;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CONFIGURATION_FILE_NAME;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CRYPTO_LOADBALANCER_PASSWORD;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CRYPTO_LOADBALANCER_PKCS12_FILE;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CRYPTO_REST_API_PASSWORD;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CRYPTO_REST_API_PKCS12_FILE;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CRYPTO_ZOOKEEPER_PASSWORD;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CRYPTO_ZOOKEEPER_PKCS12_FILE;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.REST_API_IP_ADDRESS;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.REST_API_PORT;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.RUNNING_MODE;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.ZOOKEEPER_CONNECTION_STRING;
import static com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil.getPropertyOrEnv;
import static org.junit.jupiter.api.Assertions.*;

class BootstrapTest {

    @BeforeAll
    static void loadConfigurationFile() {
        assertNull(getPropertyOrEnv(CONFIGURATION_DIRECTORY.name()));

        ClassLoader classLoader = BootstrapTest.class.getClassLoader();
        File file = new File(classLoader.getResource("default").getFile());
        String absolutePath = file.getAbsolutePath();

        System.setProperty(CONFIGURATION_DIRECTORY.name(), absolutePath);
        assertNotNull(getPropertyOrEnv(CONFIGURATION_DIRECTORY.name()));
    }

    @AfterEach
    void shutdownBootstrapInstance() {
        Bootstrap.shutdown();
    }

    @Test
    void loadConfigurationFileAndCheckSystemPropertiesTest() throws Exception {
        Bootstrap.main();

        assertEquals(RunningMode.STANDALONE, RunningMode.valueOf(getPropertyOrEnv(RUNNING_MODE.name())));
        assertEquals("1-2-3-4-5-f", getPropertyOrEnv(CLUSTER_ID.name()));

        assertEquals("127.0.0.1", getPropertyOrEnv(REST_API_IP_ADDRESS.name()));
        assertEquals("9110", getPropertyOrEnv(REST_API_PORT.name()));

        assertEquals("", getPropertyOrEnv(ZOOKEEPER_CONNECTION_STRING.name()));

        assertEquals("target/test-classes/default/restapi.p12", getPropertyOrEnv(CRYPTO_REST_API_PKCS12_FILE.name()));
        assertEquals("shieldblaze", getPropertyOrEnv(CRYPTO_REST_API_PASSWORD.name()));

        assertEquals("target/test-classes/default/zookeeper.p12", getPropertyOrEnv(CRYPTO_ZOOKEEPER_PKCS12_FILE.name()));
        assertEquals("expressgateway", getPropertyOrEnv(CRYPTO_ZOOKEEPER_PASSWORD.name()));

        assertEquals("target/test-classes/default/loadbalancer.p12", getPropertyOrEnv(CRYPTO_LOADBALANCER_PKCS12_FILE.name()));
        assertEquals("shieldblazeexpressgateway", getPropertyOrEnv(CRYPTO_LOADBALANCER_PASSWORD.name()));
    }

    @Test
    void loadConfigurationAndUseEnforcingTest() {
        System.setProperty(CONFIGURATION_FILE_NAME.name(), "enforcingConfiguration.json");
        assertThrows(NullPointerException.class, Bootstrap::main);
    }

    @Test
    void loadConfigurationAndUseNonEnforcingTest() {
        System.setProperty(CONFIGURATION_FILE_NAME.name(), "nonEnforcingConfiguration.json");
        System.setProperty(RUNNING_MODE.name(), "STANDALONE");

        assertDoesNotThrow(() -> Bootstrap.main());
        assertEquals(RunningMode.STANDALONE.name(), getPropertyOrEnv(RUNNING_MODE.name()));
    }
}
