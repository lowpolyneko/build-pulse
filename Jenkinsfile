pipeline {
    agent any
    environment {
        PATH = '/nfs/gce/projects/pmrs/opt/cargo/bin:${env.PATH}'
    }
    stages {
        stage('prepare') {
            steps {
                copyArtifacts projectName: currentBuild.fullProjectName, selector: lastCompleted(), excludes: 'build-pulse', optional: true
            }
        }
        stage('build') {
            steps {
                sh 'cargo build --release'
            }
        }
        stage('deploy') {
            steps {
                sh './target/release/build-pulse -o report.html'
            }
        }
        stage('package') {
            steps {
                archiveArtifacts artifacts: 'target/release/build-pulse,report.html,data.db,config.toml'
            }
        }
    }
}

// vim: ts=4:sw=4:expandtab:
