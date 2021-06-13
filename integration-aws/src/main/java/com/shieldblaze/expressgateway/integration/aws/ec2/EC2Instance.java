/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
package com.shieldblaze.expressgateway.integration.aws.ec2;

import com.shieldblaze.expressgateway.integration.Server;
import com.shieldblaze.expressgateway.integration.event.ServerDestroyEvent;
import com.shieldblaze.expressgateway.integration.event.ServerRestartEvent;
import software.amazon.awssdk.services.ec2.model.Instance;
import software.amazon.awssdk.services.ec2.model.InstanceNetworkInterface;
import software.amazon.awssdk.services.ec2.model.RunInstancesResponse;
import software.amazon.awssdk.services.ec2.model.Tag;

import java.net.Inet4Address;
import java.net.Inet6Address;
import java.net.InetAddress;

public class EC2Instance implements Server {

    private String id;
    private long startTime;
    private boolean autoscaled;
    private boolean inUse;
    private Inet4Address inet4Address;
    private Inet6Address inet6Address;

    @Override
    public String id() {
        return id;
    }

    @Override
    public long startTime() {
        return startTime;
    }

    @Override
    public boolean autoscaled() {
        return autoscaled;
    }

    @Override
    public boolean inUse() {
        return inUse;
    }

    public void inUse(boolean inUse) {
        this.inUse = inUse;
    }

    @Override
    public Inet4Address ipv4Address() {
        return inet4Address;
    }

    @Override
    public Inet6Address ipv6Address() {
        return inet6Address;
    }

    @Override
    public ServerRestartEvent restart() {
        return null;
    }

    @Override
    public ServerDestroyEvent destroy() {
        return null;
    }

    @Override
    public String providerName() {
        return "AWS-EC2";
    }

    @Override
    public String toString() {
        return "EC2Instance{" +
                "id='" + id + '\'' +
                ", startTime=" + startTime +
                ", autoscaled=" + autoscaled +
                ", inUse=" + inUse +
                ", inet4Address=" + inet4Address +
                ", inet6Address=" + inet6Address +
                '}';
    }

    /**
     * Build {@link EC2Instance} Instance from {@link Instance}
     */
    public static EC2Instance buildFrom(Instance instance) {

        if (!instance.hasTags()) {
            throw new IllegalArgumentException("Instance does not have any tags");
        }

        boolean foundTag = false;
        boolean autoscaled = false;
        for (Tag tag : instance.tags()) {
            if (tag.key().equalsIgnoreCase("ExpressGateway")) {
                if (tag.value().equalsIgnoreCase("Master")) {
                    foundTag = true;
                    break;
                } else if (tag.value().equalsIgnoreCase("Autoscaled")) {
                    autoscaled = true;
                    foundTag = true;
                    break;
                }
            }
        }

        if (!foundTag) {
            throw new IllegalArgumentException("Instance does not have valid tags");
        }

        EC2Instance ec2Instance = new EC2Instance();
        ec2Instance.id = instance.instanceId();
        ec2Instance.startTime = instance.launchTime().toEpochMilli();
        ec2Instance.autoscaled = autoscaled;

        try {
            ec2Instance.inet4Address = (Inet4Address) InetAddress.getByName(instance.publicIpAddress());
            ec2Instance.inet6Address = (Inet6Address) InetAddress.getByName(instance.networkInterfaces().get(0).ipv6Addresses().get(0).ipv6Address());
        } catch (Exception ex) {
            // This should never happen
        }

        return ec2Instance;
    }
}
