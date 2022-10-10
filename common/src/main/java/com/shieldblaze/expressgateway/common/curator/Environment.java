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
package com.shieldblaze.expressgateway.common.curator;

import com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil;

import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.RUNTIME_ENVIRONMENT;

/**
 * Environments for ZooKeeper
 */
public enum Environment {
    DEVELOPMENT("dev"),
    QUALITY_ASSURANCE("qa"),
    PRODUCTION("prod");

    private final String env;

    Environment(String env) {
        this.env = env;
    }

    public String env() {
        return env;
    }

    /**
     * Automatically detect environment from System Property or System environment variable.
     * Default value is {@link #PRODUCTION}.
     *
     * @return {@link Environment} type
     */
    public static Environment detectEnv() {
        String env = SystemPropertyUtil.getPropertyOrEnv(RUNTIME_ENVIRONMENT.name(), PRODUCTION.env()).toLowerCase();
        return switch (env) {
            case "dev", "development" -> DEVELOPMENT;
            case "qa", "quality-assurance" -> QUALITY_ASSURANCE;
            case "prod", "production" -> PRODUCTION;
            default -> null;
        };
    }

    /**
     * Set environment in System Property
     *
     * @param environment {@link Environment} to set
     */
    public static void setEnvironment(Environment environment) {
        System.setProperty(RUNTIME_ENVIRONMENT.name(), environment.env().toLowerCase());
    }
}
