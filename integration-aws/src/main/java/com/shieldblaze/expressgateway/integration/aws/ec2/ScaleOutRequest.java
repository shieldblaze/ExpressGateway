package com.shieldblaze.expressgateway.integration.aws.ec2;

import software.amazon.awssdk.services.ec2.model.InstanceType;

import java.util.List;

public final class ScaleOutRequest {

    private final String imageId;
    private final String subnetId;
    private final InstanceType instanceType;
    private final List<String> securityGroups;
    private final boolean autoscaled;

    ScaleOutRequest(String imageId, String subnetId, InstanceType instanceType, List<String> securityGroups, boolean autoscaled) {
        this.imageId = imageId;
        this.subnetId = subnetId;
        this.instanceType = instanceType;
        this.securityGroups = securityGroups;
        this.autoscaled = autoscaled;
    }

    public String imageId() {
        return imageId;
    }

    public String subnetId() {
        return subnetId;
    }

    public InstanceType instanceType() {
        return instanceType;
    }

    public List<String> securityGroups() {
        return securityGroups;
    }

    public boolean autoscaled() {
        return autoscaled;
    }
}
