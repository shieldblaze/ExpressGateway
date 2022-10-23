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
package com.shieldblaze.expressgateway.common.zookeeper;

import java.util.Objects;
import java.util.UUID;

/**
 * This class builds ZNode path for ZooKeeper
 */
public final class ZNodePath {

    private final String path;

    private ZNodePath(String path) {
        this.path = Objects.requireNonNull(path, "ZNode path cannot be 'null'");
    }

    /**
     * Create a new {@link ZNodePath} instance
     *
     * @param rootPath Root level path name
     * @return {@link ZNodePath} instance
     */
    public static ZNodePath create(String rootPath) {
        return create(rootPath, null, null, null);
    }

    /**
     * Create a new {@link ZNodePath} instance
     *
     * @param rootPath    Root level path name
     * @param environment {@link Environment} to use
     * @return {@link ZNodePath} instance
     */
    public static ZNodePath create(String rootPath, Environment environment) {
        return create(rootPath, environment, null, null);
    }

    /**
     * Create a new {@link ZNodePath} instance
     *
     * @param rootPath    Root level path name
     * @param environment {@link Environment} to use
     * @param id          {@link UUID} to use
     * @return {@link ZNodePath} instance
     */
    public static ZNodePath create(String rootPath, Environment environment, String id) {
        return create(rootPath, environment, id, null);
    }

    /**
     * Create a new {@link ZNodePath} instance
     *
     * @param rootPath    Root level path name
     * @param environment {@link Environment} to use
     * @param id          {@link UUID} to use
     * @param component   Component name
     * @return {@link ZNodePath} instance
     */
    public static ZNodePath create(String rootPath, Environment environment, String id, String component) {
        return new ZNodePath(buildZNodePath(rootPath, environment, id, component, null));
    }

    /**
     * Create a new {@link ZNodePath} instance
     *
     * @param rootPath    Root level path name
     * @param environment {@link Environment} to use
     * @param id          {@link UUID} to use
     * @param component   Component name
     * @param friendlyName Friendly name
     * @return {@link ZNodePath} instance
     */
    public static ZNodePath create(String rootPath, Environment environment, String id, String component, String friendlyName) {
        return new ZNodePath(buildZNodePath(rootPath, environment, id, component, friendlyName));
    }

    private static String buildZNodePath(String rootPath, Environment environment, String id, String component, String friendlyName) {
        /*
         * Example:
         *          /RootPath
         *          /RootPath/Prod
         *          /RootPath/Prod/1-2-3-4-5-f
         *          /RootPath/Prod/1-2-3-4-5-f/TransportConfig
         *          /RootPath/Prod/1-2-3-4-5-f/TransportConfig/Default
         */
        StringBuilder path = new StringBuilder()
                .append('/').append(rootPath);

        if (environment != null) {
            path.append('/').append(environment.env());
        }

        if (id != null) {
            path.append('/').append(id);
        }

        if (component != null) {
            path.append('/').append(component);
        }

        if (friendlyName != null) {
            path.append('/').append(friendlyName);
        }

        return path.toString();
    }

    public String path() {
        return path;
    }

    @Override
    public String toString() {
        return "ZNodePath{" +
                "path='" + path + '\'' +
                '}';
    }
}
