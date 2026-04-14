import toml
import sys
import os

def update_pyproject(filepath, project_name, project_version, package_dir_name):
    # Read the pyproject.toml file
    with open(filepath, 'r') as f:
        data = toml.load(f)

    # Update project name and version
    data['project']['name'] = project_name
    data['project']['version'] = project_version

    # Ensure tool.hatch.build.targets.wheel section exists
    if 'tool' not in data:
        data['tool'] = {}
    if 'hatch' not in data['tool']:
        data['tool']['hatch'] = {}
    if 'build' not in data['tool']['hatch']:
        data['tool']['hatch']['build'] = {}
    if 'targets' not in data['tool']['hatch']['build']:
        data['tool']['hatch']['build']['targets'] = {}
    if 'wheel' not in data['tool']['hatch']['build']['targets']:
        data['tool']['hatch']['build']['targets']['wheel'] = {}

    # Set the packages key to the actual directory name
    data['tool']['hatch']['build']['targets']['wheel']['packages'] = [package_dir_name]

    # Write the updated pyproject.toml file
    with open(filepath, 'w') as f:
        toml.dump(data, f)

if __name__ == '__main__':
    if len(sys.argv) != 5:
        print("Usage: python update_pyproject.py <filepath> <project_name> <project_version> <package_dir_name>")
        sys.exit(1)

    filepath = sys.argv[1]
    project_name = sys.argv[2]
    project_version = sys.argv[3]
    package_dir_name = sys.argv[4]

    # Restore original pyproject.toml from git before modification
    os.system(f"git restore {filepath} || true")

    update_pyproject(filepath, project_name, project_version, package_dir_name)
    print(f"Updated {filepath} for project {project_name} version {project_version} with package dir {package_dir_name}")
