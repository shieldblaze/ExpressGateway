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

import org.apache.curator.framework.CuratorFramework;
import org.apache.zookeeper.CreateMode;

public final class CuratorUtils {

    /**
     * Create a new ZNode and set the data
     *
     * @param curatorFramework {@link CuratorFramework} instance
     * @param zNodePath        {@link ZNodePath} instance
     * @param data             Data as {@link Byte} array
     * @return Returns {@link Boolean#TRUE} if operation was successful else {@link Boolean#FALSE}
     * @throws Exception In case of an error during creation
     */
    public static boolean createNew(CuratorFramework curatorFramework, ZNodePath zNodePath, byte[] data) throws Exception {
        return createNew(curatorFramework, zNodePath, data, false);
    }

    /**
     * Create a new ZNode and set the data
     *
     * @param curatorFramework    {@link CuratorFramework} instance
     * @param zNodePath           {@link ZNodePath} instance
     * @param data                Data as {@link Byte} array
     * @param setDataIfPathExists Set to {@link Boolean#TRUE} if we should just set the data if path already exists
     * @return Returns {@link Boolean#TRUE} if operation was successful else {@link Boolean#FALSE}
     * @throws Exception In case of an error during creation
     */
    public static boolean createNew(CuratorFramework curatorFramework, ZNodePath zNodePath, byte[] data, boolean setDataIfPathExists) throws Exception {
        if (setDataIfPathExists) {
            return curatorFramework.create()
                    .orSetData()
                    .creatingParentsIfNeeded()
                    .withMode(CreateMode.PERSISTENT)
                    .forPath(zNodePath.path(), data) != null;
        } else {
            return curatorFramework.create()
                    .creatingParentsIfNeeded()
                    .withMode(CreateMode.PERSISTENT)
                    .forPath(zNodePath.path(), data) != null;
        }
    }

    /**
     * Check if a path exists
     *
     * @param curatorFramework {@link CuratorFramework} instance
     * @param zNodePath        {@link ZNodePath} instance
     * @return Returns {@link Boolean#TRUE} if path exists else returns {@link Boolean#FALSE}
     * @throws Exception In case of an error during checking
     */
    public static boolean doesPathExists(CuratorFramework curatorFramework, ZNodePath zNodePath) throws Exception {
        return curatorFramework.checkExists().forPath(zNodePath.path()) != null;
    }

    /**
     * Get data as {@link Byte} array
     *
     * @param curatorFramework {@link CuratorFramework} instance
     * @param zNodePath        {@link ZNodePath} instance
     * @return Data as {@link Byte} array
     * @throws Exception In case of an error during getting data
     */
    public static byte[] getData(CuratorFramework curatorFramework, ZNodePath zNodePath) throws Exception {
        return curatorFramework.getData().forPath(zNodePath.path());
    }

    /**
     * Set data as {@link Byte} array
     *
     * @param curatorFramework {@link CuratorFramework} instance
     * @param zNodePath        {@link ZNodePath} instance
     * @param data             Data as {@link Byte} array
     * @return Returns {@link Boolean#TRUE} if data is set successfully else returns {@link Boolean#FALSE}
     * @throws Exception In case of an error during getting data
     */
    public static boolean setData(CuratorFramework curatorFramework, ZNodePath zNodePath, byte[] data) throws Exception {
        return curatorFramework.setData().forPath(zNodePath.path(), data) != null;
    }

    /**
     * Delete ZNode
     *
     * @param curatorFramework {@link CuratorFramework} instance
     * @param zNodePath        {@link ZNodePath} instance
     * @throws Exception In case of an error during deleting data
     */
    public static void deleteData(CuratorFramework curatorFramework, ZNodePath zNodePath) throws Exception {
        deleteData(curatorFramework, zNodePath, false);
    }

    /**
     * Delete ZNode
     *
     * @param curatorFramework {@link CuratorFramework} instance
     * @param zNodePath        {@link ZNodePath} instance
     * @param deleteChild      Deletes all child ZNodes if set to {@link Boolean#TRUE}
     * @throws Exception In case of an error during deleting data
     */
    public static void deleteData(CuratorFramework curatorFramework, ZNodePath zNodePath, boolean deleteChild) throws Exception {
        if (deleteChild) {
            curatorFramework.delete().deletingChildrenIfNeeded().forPath(zNodePath.path());
        } else {
            curatorFramework.delete().forPath(zNodePath.path());
        }
    }

    private CuratorUtils() {
        // Prevent outside initialization
    }
}
