extern crate git2;
extern crate clap;

use git2::*;
use std::env;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::exit;

const CENTRAL_NAME:    &'static str = "bsw/central";
const AUTOMATION_USER: &'static str = "automation";

// FIXME: hardcoded path
const TMP_REPO_DIR:    &'static str = "/home/christian/gerrit_testsite/tmp_automation_repo";

fn module_review_upload(project: &str, newrev: &str) {

  let tmp_repo = Repository::init_bare(TMP_REPO_DIR).unwrap();
  let _ = in_tmp_repo("fetch --all");

  transfer_to_tmp(newrev);
  let parent_commit_obj = tmp_repo.revparse_single(CENTRAL_NAME).unwrap();
  let mut parent_commit_oid = parent_commit_obj.as_commit().unwrap().id();

  let module_name = Path::new(project).components().last().unwrap();
  let s = module_name.as_ref().to_str().unwrap();
  let oldrev = format!(
    "{}",tmp_repo.revparse_single(&format!("remotes/modules/{}/master",s)).unwrap().id()
    );

  {
    let old = tmp_repo.revparse_single(&oldrev).unwrap().id();
    let new = tmp_repo.revparse_single(&newrev).unwrap().id();

    if !tmp_repo.graph_descendant_of(new,old).unwrap() {
      println!(".");
      println!("===========================================================");
      println!("======== Commit not based on master, rebase first! ========");
      println!("===========================================================");
      return;
    }
  }

  let walk = {
    let mut walk = tmp_repo.revwalk().unwrap();
    walk.set_sorting( Sort::from_bits(5).unwrap());
    let _ = walk.push_range(&format!("{}..{}", oldrev, newrev));
    walk
  };


  println!("===== project path: {}", project);
  println!("===== Apply commits from {} to {}", oldrev, newrev);

  for rev in walk {
    let newrev = format!("{}",rev.unwrap());
    if oldrev == newrev { continue; }
    println!("===== Apply commit {}", newrev);

    let module_commit_obj = tmp_repo.revparse_single(&newrev).unwrap();
    let module_commit = module_commit_obj.as_commit().unwrap();
    let module_tree = module_commit.tree().unwrap();

    let parent_commit = tmp_repo.find_commit(parent_commit_oid).unwrap();

    let new_tree = {
      let master_tree = parent_commit.tree().unwrap();
      let new_tree_oid = module_to_subfolder(Path::new(module_name.as_ref()), &module_tree, &master_tree);
      tmp_repo.find_tree(new_tree_oid).unwrap()
    };

    parent_commit_oid = make_commit(&tmp_repo, &new_tree, module_commit, &vec!(&parent_commit)).unwrap();

  }

  println!(""); println!("");
  println!("===================== Doing actual upload in central git ========================");
  let x = push_from_tmp(
    &tmp_repo,
    &tmp_repo.find_commit(parent_commit_oid).unwrap(),
    CENTRAL_NAME,
    "refs/for/master"
    ).unwrap();
  println!("{}", x);
  println!("==== The review upload may have worked, even if it says error below. Look UP! ====");
}

fn central_submit(newrev: &str) {
  println!("central_submit");

  let module_names = get_module_names(newrev);
  let tmp_repo = setup_tmp_repo(&module_names);
  transfer_to_tmp(newrev);

  let central_commit_obj = tmp_repo.revparse_single(newrev).unwrap();
  let central_commit = central_commit_obj.as_commit().unwrap();
  let central_tree = central_commit.tree().unwrap();
  let _ = tmp_repo.branch(CENTRAL_NAME,central_commit,true);

  for module_name in module_names {
    let module_master_commit_obj =
      tmp_repo.revparse_single(&format!("remotes/modules/{}/master",module_name)).unwrap();
    let module_master_commit =
      module_master_commit_obj.as_commit().unwrap();
    let _ = tmp_repo.branch(&format!("modules/{}",module_name),module_master_commit,true);

    let parents = vec!(module_master_commit);
    let old_tree_oid = module_master_commit.tree().unwrap().id();

    let module_path = { let mut p = PathBuf::new(); p.push("modules"); p.push(&module_name); p };

    let new_tree_oid = central_tree.get_path(&module_path).unwrap().id();

    if new_tree_oid != old_tree_oid {

      let new_tree = tmp_repo.find_tree(new_tree_oid).unwrap();

      let module_commit = make_commit(&tmp_repo, &new_tree, central_commit, &parents);
      let x = push_from_tmp(
        &tmp_repo,
        &tmp_repo.find_commit(module_commit.unwrap()).unwrap(),
        &format!("bsw/modules/{}",module_name),
        "master"
        ).unwrap();
      println!("{}", x);
    }
  }
}

fn transfer_to_tmp(rev: &str) {
  Command::new("git")
    .arg("branch").arg("-f").arg("tmp").arg(rev)
    .output().expect("failed to call git");

  Command::new("git")
    .arg("push").arg("--force").arg(TMP_REPO_DIR).arg("tmp")
    .output().expect("failed to call git");

  Command::new("git")
    .arg("branch").arg("-D").arg("tmp")
    .output().expect("failed to call git");
}

fn in_tmp_repo(cmd: &str) -> Result<String, std::io::Error> {
  let args: Vec<&str> = cmd.split(" ").collect();
  Command::new("git")
    .env("GIT_DIR",TMP_REPO_DIR)
    .args(&args).output().map(|output|
                              format!("{}", String::from_utf8_lossy(&output.stderr)))
}

fn setup_tmp_repo(modules: &Vec<String>) -> Repository {
  let repo = Repository::init_bare(TMP_REPO_DIR).unwrap();

  if !repo.find_remote("central_repo").is_ok() {
    let _ = repo.remote("central_repo",
                        &format!("ssh://{}@gerrit-test-git/{}.git",AUTOMATION_USER,CENTRAL_NAME)
                       );
  }

  for module in modules.iter() {
    let output = Command::new("ssh")
      .arg("-p").arg("29418")
      .arg("gerrit-test-git")
      .arg("gerrit")
      .arg("create-project")
      .arg(format!("bsw/modules/{}",module))
      .arg("--empty-commit")
      .output()
      .expect("failed to create project");

    println!("create-project: {}", String::from_utf8_lossy(&output.stderr));

    let remote_url = format!("ssh://{}@gerrit-test-git:29418/bsw/modules/{}.git",
                             AUTOMATION_USER,
                             module
                            );

    let remote_name = format!("modules/{}",module);
    if !repo.find_remote(&remote_name).is_ok() {
      let _ = repo.remote(&remote_name, &remote_url);
    }
  }

  let _ = in_tmp_repo("fetch --all");

  return repo;
}

fn module_to_subfolder(path: &Path, module_tree: &Tree, master_tree: &Tree) -> Oid {
  let mpath = Path::new("modules");
  let modules_oid = master_tree.get_path(mpath).unwrap().id();
  let tmp_repo = Repository::init_bare(TMP_REPO_DIR).unwrap();

  let modules_tree = tmp_repo.find_tree(modules_oid).unwrap();
  let mut mbuilder = tmp_repo.treebuilder(Some(&modules_tree)).unwrap();
  mbuilder.insert(path, module_tree.id(), 0o0040000).expect("mbuilder insert failed"); // GIT_FILEMODE_TREE
  let mtree = mbuilder.write().unwrap();

  let mut builder = tmp_repo.treebuilder(Some(master_tree)).unwrap();
  builder.insert(mpath, mtree, 0o0040000).expect("builder insert failed"); // GIT_FILEMODE_TREE
  let r = builder.write().unwrap();
  println!("module_to_subfolder {}", r);
  return r;
}

fn get_module_names(rev: &str) -> Vec<String> {
  let central_repo = Repository::open(".").unwrap();

  let object = central_repo.revparse_single(rev).unwrap();
  let commit = object.as_commit().unwrap();
  let tree = commit.tree().unwrap();

  let modules_o = tree.get_path(&Path::new("modules")).unwrap()
    .to_object(&central_repo).unwrap();
  let modules = modules_o.as_tree().unwrap();

  let mut names = Vec::<String>::new();
  for module in modules.iter() {
    names.push(module.name().unwrap().to_string());
  }
  return names;
}

fn push_from_tmp(tmp_repo: &Repository,
                 commit: &Commit,
                 repo: &str ,to: &str)
  -> Result<String, std::io::Error> {
    let _ = tmp_repo.set_head_detached(commit.id());
    in_tmp_repo(
      &format!("push ssh://{}@gerrit-test-git:29418/{}.git HEAD:{}",
               AUTOMATION_USER,
               repo,
               to
              )
      )
  }

fn make_commit(repo: &Repository, tree: &Tree, base: &Commit, parents: &[&Commit]) -> Option<Oid> {
  if parents.len() != 0 {
    let _ = repo.set_head_detached(parents[0].id());
  }
  repo.commit(
    Some("HEAD"),
    &base.author(),
    &base.committer(),
    &base.message().unwrap_or("no message"),
    tree,
    parents
    ).ok()
}

fn main() { exit(main_ret()); } fn main_ret() -> i32 {

  let args = clap::App::new("centralgithook")
    .arg(clap::Arg::with_name("oldrev").long("oldrev").takes_value(true))
    .arg(clap::Arg::with_name("newrev").long("newrev").takes_value(true))
    .arg(clap::Arg::with_name("project").long("project").takes_value(true))
    .arg(clap::Arg::with_name("refname").long("refname").takes_value(true))
    .arg(clap::Arg::with_name("uploader").long("uploader").takes_value(true))
    .arg(clap::Arg::with_name("commit").long("commit").takes_value(true))
    .arg(clap::Arg::with_name("change").long("change").takes_value(true))
    .arg(clap::Arg::with_name("change-url").long("change-url").takes_value(true))
    .arg(clap::Arg::with_name("change-owner").long("change-owner").takes_value(true))
    .arg(clap::Arg::with_name("branch").long("branch").takes_value(true))
    .arg(clap::Arg::with_name("submitter").long("submitter").takes_value(true))
    .arg(clap::Arg::with_name("topic").long("topic").takes_value(true))
    .get_matches();

  let newrev = args.value_of("newrev").unwrap_or("");
  let project = args.value_of("project").unwrap_or("");
  let refname = args.value_of("refname").unwrap_or("");
  let commit = args.value_of("commit").unwrap_or("");


  if let Some(hook) = env::args().nth(0) {
    let is_review = refname == "refs/for/master";
    let is_module = project != CENTRAL_NAME;
    let is_update = hook.ends_with("ref-update");
    let is_submit = hook.ends_with("change-merged");

    // // TODO
    // let uploader = args.value_of("uploader").unwrap_or("");
    // if !is_review && !uploader.contains("Automation") {
    //   println!("only push to refs/for/master");
    //   return 1;
    // }

    if is_submit { central_submit(commit); }
    else if !is_module && is_update && !is_review { central_submit(newrev); }
    else if is_module && is_update && is_review { module_review_upload(project,newrev); return 1; }
  }

  return 0;
}


