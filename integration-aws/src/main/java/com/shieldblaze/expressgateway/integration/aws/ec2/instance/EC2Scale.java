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
package com.shieldblaze.expressgateway.integration.aws.ec2.instance;

import com.shieldblaze.expressgateway.common.annotation.Async;
import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.integration.aws.ec2.events.EC2ScaleInEvent;
import com.shieldblaze.expressgateway.integration.aws.ec2.events.EC2ScaleOutEvent;
import com.shieldblaze.expressgateway.integration.server.ScaleIn;
import com.shieldblaze.expressgateway.integration.server.ScaleOut;
import software.amazon.awssdk.services.ec2.Ec2Client;
import software.amazon.awssdk.services.ec2.model.DescribeInstancesRequest;
import software.amazon.awssdk.services.ec2.model.DescribeInstancesResponse;
import software.amazon.awssdk.services.ec2.model.Instance;
import software.amazon.awssdk.services.ec2.model.InstanceStateChange;
import software.amazon.awssdk.services.ec2.model.InstanceStateName;
import software.amazon.awssdk.services.ec2.model.ResourceType;
import software.amazon.awssdk.services.ec2.model.RunInstancesRequest;
import software.amazon.awssdk.services.ec2.model.RunInstancesResponse;
import software.amazon.awssdk.services.ec2.model.Tag;
import software.amazon.awssdk.services.ec2.model.TagSpecification;
import software.amazon.awssdk.services.ec2.model.TerminateInstancesRequest;
import software.amazon.awssdk.services.ec2.model.TerminateInstancesResponse;

public final class EC2Scale implements ScaleIn<EC2Instance, EC2ScaleInEvent>, ScaleOut<ScaleOutRequest, EC2ScaleOutEvent> {

    private final Ec2Client ec2Client;

    public EC2Scale(Ec2Client ec2Client) {
        this.ec2Client = ec2Client;
    }

    @Async
    @Override
    public EC2ScaleInEvent scaleIn(EC2Instance ec2Instance) {
        EC2ScaleInEvent event = new EC2ScaleInEvent();

        GlobalExecutors.submitTask(() -> {
            try {
                TerminateInstancesRequest terminateInstancesRequest = TerminateInstancesRequest.builder()
                        .instanceIds(ec2Instance.id())
                        .build();

                TerminateInstancesResponse terminateInstancesResponse = ec2Client.terminateInstances(terminateInstancesRequest);
                boolean isTerminated = !terminateInstancesResponse.terminatingInstances().isEmpty();

                if (!isTerminated) {
                    event.markFailure(new IllegalArgumentException("Instance was not terminated"));
                }

                InstanceStateChange instanceStateChange = terminateInstancesResponse.terminatingInstances().get(0);
                InstanceStateName instanceStateName = instanceStateChange.currentState().name();

                if (isTerminated && (instanceStateName == InstanceStateName.SHUTTING_DOWN || instanceStateName == InstanceStateName.TERMINATED)) {
                    event.markSuccess(terminateInstancesResponse);
                } else {
                    event.markFailure(new IllegalArgumentException("Unknown InstanceState: " + instanceStateName));
                }
            } catch (Exception ex) {
                event.markFailure(ex);
            }
        });

        event.future().whenCompleteAsync((response, throwable) -> {
            if (throwable == null) {

            }
        }, GlobalExecutors.executorService());

        return event;
    }

    @Async
    @Override
    public EC2ScaleOutEvent scaleOut(ScaleOutRequest scaleOutRequest) {
        EC2ScaleOutEvent event = new EC2ScaleOutEvent();

        GlobalExecutors.submitTask(() -> {
            String instanceId = null;

            try {
                RunInstancesRequest runInstancesRequest = RunInstancesRequest.builder()
                        .imageId(scaleOutRequest.imageId())
                        .subnetId(scaleOutRequest.subnetId())
                        .instanceType(scaleOutRequest.instanceType())
                        .securityGroupIds(scaleOutRequest.securityGroups())
                        .tagSpecifications(TagSpecification.builder().resourceType(ResourceType.INSTANCE).tags(
                                Tag.builder().key("ExpressGateway").value(scaleOutRequest.autoscaled() ? "Autoscaled" : "Master").build()
                        ).build())
                        .minCount(1)
                        .maxCount(1)
                        .ipv6AddressCount(1)
                        .ebsOptimized(true)
                        .build();

                RunInstancesResponse runInstancesResponse = ec2Client.runInstances(runInstancesRequest);
                if (!runInstancesResponse.hasInstances()) {
                    throw new IllegalArgumentException("RunInstancesResponse does not have any Instances");
                }

                Instance instance = runInstancesResponse.instances().get(0);
                instanceId = instance.instanceId();

                // Wait for 10 seconds for Instance to become Ready
                Thread.sleep(1000 * 10);

                DescribeInstancesResponse describeInstancesResponse = ec2Client.describeInstances(DescribeInstancesRequest.builder()
                        .instanceIds(instanceId)
                        .build());

                instance = describeInstancesResponse.reservations().get(0).instances().get(0);
                event.markSuccess(instance);
            } catch (Exception ex) {
                try {
                    if (instanceId != null) {
                        ec2Client.terminateInstances(TerminateInstancesRequest.builder()
                                .instanceIds(instanceId)
                                .build());
                    }
                } catch (Exception e) {
                    // We can ignore this safely
                }

                event.markFailure(ex);
            }
        });

        return event;
    }
}
