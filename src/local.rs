use tokio::process::Command;

use crate::config::Config;

/// Map plan machine_type to dd-vm.sh size preset.
pub fn plan_to_vm_size(machine_type: &str) -> &'static str {
    match machine_type {
        "kvm-tiny" => "tiny",
        "kvm-small" => "small",
        "kvm-medium" => "medium",
        "kvm-large" => "large",
        _ => "small",
    }
}

pub async fn create_vm(
    config: &Config,
    vm_name: &str,
    size: &str,
    github_handle: &str,
) -> Result<(), String> {
    let host = config
        .baremetal_host
        .as_deref()
        .ok_or("BAREMETAL_HOST not configured")?;
    let user = &config.baremetal_user;
    let register_url = &config.dd_register_url;

    let cmd = format!(
        "dd-vm.sh create --name {vm_name} --size {size} --env DD_OWNER={github_handle} --env DD_REGISTER_URL={register_url}"
    );

    let output = Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "ConnectTimeout=10",
            &format!("{user}@{host}"),
            &cmd,
        ])
        .output()
        .await
        .map_err(|e| format!("ssh failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("dd-vm.sh create failed: {stderr}"));
    }

    Ok(())
}

pub async fn destroy_vm(config: &Config, vm_name: &str) -> Result<(), String> {
    let host = config
        .baremetal_host
        .as_deref()
        .ok_or("BAREMETAL_HOST not configured")?;
    let user = &config.baremetal_user;

    let output = Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "ConnectTimeout=10",
            &format!("{user}@{host}"),
            &format!("dd-vm.sh destroy --name {vm_name}"),
        ])
        .output()
        .await
        .map_err(|e| format!("ssh failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("dd-vm.sh destroy failed: {stderr}"));
    }

    Ok(())
}

/// Reassign a warm VM to a new owner by restarting dd-agent with new DD_OWNER.
pub async fn reassign_vm(
    config: &Config,
    vm_name: &str,
    github_handle: &str,
) -> Result<(), String> {
    let host = config
        .baremetal_host
        .as_deref()
        .ok_or("BAREMETAL_HOST not configured")?;
    let user = &config.baremetal_user;
    let register_url = &config.dd_register_url;

    // Get the VM's IP, SSH in, kill old dd-agent, restart with new owner
    let cmd = format!(
        r#"VM_IP=$(virsh domifaddr dd-vm-{vm_name} 2>/dev/null | grep -oP '(\d+\.)+\d+' | head -1) && \
        ssh -o StrictHostKeyChecking=no "root@$VM_IP" \
        'pkill dd-agent; sleep 1; DD_OWNER={github_handle} DD_REGISTER_URL={register_url} nohup /usr/local/bin/dd-agent > /var/log/dd-agent.log 2>&1 &'"#
    );

    let output = Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "ConnectTimeout=10",
            &format!("{user}@{host}"),
            &cmd,
        ])
        .output()
        .await
        .map_err(|e| format!("ssh failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("reassign failed: {stderr}"));
    }

    Ok(())
}

pub async fn check_capacity(config: &Config) -> Result<bool, String> {
    let host = match config.baremetal_host.as_deref() {
        Some(h) => h,
        None => return Ok(false),
    };
    let user = &config.baremetal_user;

    let output = Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "ConnectTimeout=10",
            &format!("{user}@{host}"),
            "virsh list --all | grep -c 'dd-vm-' || echo 0",
        ])
        .output()
        .await
        .map_err(|e| format!("ssh failed: {e}"))?;

    let count: i64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0);

    // Allow up to 20 VMs on local baremetal
    Ok(count < 20)
}
