use cctui_orchestrator::WorkerProfile;
use kube::CustomResourceExt;

fn main() {
    print!("{}", serde_yaml::to_string(&WorkerProfile::crd()).unwrap());
}
